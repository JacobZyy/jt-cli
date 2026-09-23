use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use serde::Serialize;
use serde_json::Value;

use super::model::{ContractIr, HttpRoute, RouteSource, RouteStatus};
use super::output::OutputLock;

const ZGATEWAY_CONFIG: &str = include_str!("../assets/zgateway.zzcli.json");

#[derive(Clone, Debug, Args)]
pub struct RoutesArgs {
    /// Generated artifact directory containing .nlab/contract-ir.json
    #[arg(long, value_name = "path")]
    project: PathBuf,
    /// ZGateway query environment
    #[arg(long, value_enum, default_value_t = GatewayEnvironment::Testserver)]
    sys_env: GatewayEnvironment,
    /// Explicitly permit online route lookup
    #[arg(long, requires = "sys_env")]
    allow_online: bool,
    /// zzcli executable
    #[arg(long, default_value = "zzcli")]
    zzcli_bin: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteSummary {
    pub replaced: usize,
    pub placeholders: usize,
    pub missing: Vec<String>,
    pub warning: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum GatewayEnvironment {
    Testserver,
    Online,
}

impl GatewayEnvironment {
    fn as_str(self) -> &'static str {
        match self {
            Self::Testserver => "testserver",
            Self::Online => "online",
        }
    }
}

pub fn run(args: RoutesArgs) -> u8 {
    match run_inner(args) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("serialize route result")
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

fn run_inner(args: RoutesArgs) -> Result<Value> {
    if args.sys_env == GatewayEnvironment::Online && !args.allow_online {
        bail!("online route lookup requires --allow-online");
    }
    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("resolve generated project {}", args.project.display()))?;
    let _lock = OutputLock::acquire(&project)?;
    let ir_path = project.join(".nlab/contract-ir.json");
    let mut ir = serde_json::from_str::<ContractIr>(
        &fs::read_to_string(&ir_path)
            .with_context(|| format!("read contract IR {}", ir_path.display()))?,
    )
    .context("decode contract IR")?;
    let summary = apply_best_effort_with_environment(&mut ir, &args.zzcli_bin, args.sys_env);
    let status = if summary.warning.is_some() || summary.placeholders > 0 {
        "complete-with-warnings"
    } else {
        "complete"
    };
    let config = super::config::ProjectConfig::load(&project)?;
    let openapi = super::openapi::generate(&ir, &config)?;
    let frontend = super::typescript::generate(&ir, &config)?;
    let written = super::output::write(&project, &ir, &openapi, &frontend)?;
    Ok(serde_json::json!({
        "status": status,
        "environment": args.sys_env.as_str(),
        "replaced": summary.replaced,
        "placeholders": summary.placeholders,
        "missing": summary.missing,
        "warning": summary.warning,
        "openapi": written.openapi_path,
        "openapiSha256": written.openapi_sha256,
        "apiFiles": written.api_files.len()
    }))
}

fn apply_best_effort_with_environment(
    ir: &mut ContractIr,
    zzcli: &Path,
    environment: GatewayEnvironment,
) -> RouteSummary {
    let routes = match query_routes(zzcli, environment, &ir.target.app_name) {
        Ok(routes) => routes,
        Err(error) => {
            return RouteSummary {
                replaced: 0,
                placeholders: ir.operations.len(),
                missing: ir
                    .operations
                    .iter()
                    .map(|operation| operation.key.clone())
                    .collect(),
                warning: Some(format!("{error:#}")),
            };
        }
    };
    let mut by_operation = BTreeMap::new();
    for route in routes {
        let Some(operation) = ir
            .operations
            .iter()
            .find(|operation| route.matches(&operation.facade_fqn, &operation.method_name))
        else {
            continue;
        };
        by_operation.insert(operation.key.clone(), route);
    }
    let mut replaced = 0usize;
    let mut missing = Vec::new();
    for operation in &mut ir.operations {
        if let Some(route) = by_operation.get(&operation.key) {
            operation.route = HttpRoute {
                status: RouteStatus::Resolved,
                source: RouteSource::Zgateway,
                method: route.method.clone(),
                path: route.path.clone(),
                host: route.host.clone(),
            };
            replaced += 1;
        } else {
            missing.push(operation.key.clone());
        }
    }
    RouteSummary {
        replaced,
        placeholders: missing.len(),
        missing,
        warning: None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HttpRouteKey {
    pub interface_name: String,
    pub method_name: String,
    pub signature: Option<String>,
    pub method: String,
    pub path: String,
    pub host: Option<String>,
    pub source: RouteSource,
}

impl HttpRouteKey {
    pub(crate) fn matches(&self, interface_name: &str, method_name: &str) -> bool {
        self.method_name == method_name
            && (self.interface_name == interface_name
                || (!self.interface_name.contains('.')
                    && interface_name.rsplit('.').next() == Some(self.interface_name.as_str())))
    }

    pub(crate) fn matches_method(
        &self,
        interface_name: &str,
        method_name: &str,
        signature: &str,
        overloaded: bool,
    ) -> bool {
        if !self.matches(interface_name, method_name) {
            return false;
        }
        if !overloaded {
            return true;
        }
        let Some(expected) = self.signature.as_deref().and_then(route_parameters) else {
            return false;
        };
        let Some((_, actual)) = crate::java::parse_method_signature(signature) else {
            return false;
        };
        expected.len() == actual.len()
            && expected
                .iter()
                .zip(actual.iter())
                .all(|(expected, actual)| same_type(expected, actual))
    }
}

fn route_parameters(signature: &str) -> Option<Vec<crate::model::TypeRef>> {
    let (_, parameters) = signature.split_once('(')?;
    let (parameters, _) = parameters.split_once(')')?;
    if parameters.trim().is_empty() {
        return Some(Vec::new());
    }
    crate::java::split_top_level(parameters, ',')
        .into_iter()
        .map(|parameter| crate::java::parse_java_type(parameter.trim()))
        .collect()
}

fn same_type(expected: &crate::model::TypeRef, actual: &crate::model::TypeRef) -> bool {
    expected.simple_name() == actual.simple_name()
        && expected.array_depth == actual.array_depth
        && expected.arguments.len() == actual.arguments.len()
        && expected
            .arguments
            .iter()
            .zip(&actual.arguments)
            .all(|(expected, actual)| same_type(expected, actual))
}

pub(crate) fn routes_for_run(
    project: &Path,
    app_name: &str,
    branch: &str,
    commit: Option<&str>,
    offline: bool,
    enabled: bool,
) -> Result<Vec<HttpRouteKey>> {
    if offline {
        let path = project.join(".nlab/contract-ir.json");
        let ir = serde_json::from_str::<ContractIr>(
            &fs::read_to_string(&path)
                .with_context(|| format!("offline gateway routes require {}", path.display()))?,
        )
        .context("decode cached contract IR")?;
        if ir.target.app_name != app_name
            || ir.target.branch != branch
            || commit.is_some_and(|commit| ir.target.commit != commit)
        {
            bail!("cached gateway routes do not match the current backend target");
        }
        return Ok(ir
            .operations
            .into_iter()
            .filter(|operation| {
                matches!(
                    operation.route.status,
                    RouteStatus::Resolved | RouteStatus::Cached
                )
            })
            .map(|operation| {
                let signature = crate::java::parse_method_signature(&operation.signature).map(
                    |(_, parameters)| {
                        format!(
                            "{}({})",
                            operation.method_name,
                            parameters
                                .iter()
                                .map(crate::model::TypeRef::render_java)
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    },
                );
                HttpRouteKey {
                    interface_name: operation.facade_fqn,
                    method_name: operation.method_name,
                    signature,
                    method: operation.route.method,
                    path: operation.route.path,
                    host: operation.route.host,
                    source: RouteSource::Cache,
                }
            })
            .collect());
    }
    if !enabled {
        bail!("gateway lookup is required for interface identification");
    }
    query_routes(Path::new("zzcli"), GatewayEnvironment::Testserver, app_name)
}

fn query_routes(
    zzcli: &Path,
    environment: GatewayEnvironment,
    app_name: &str,
) -> Result<Vec<HttpRouteKey>> {
    let config_dir = tempfile::tempdir().context("create zzcli config directory")?;
    let config = config_dir.path().join("zgateway.json");
    fs::write(&config, ZGATEWAY_CONFIG).context("write zzcli config")?;
    let output = Command::new(zzcli)
        .args(["--config"])
        .arg(&config)
        .args([
            "--sys-env",
            environment.as_str(),
            "zgateway",
            "query",
            "--appName",
            app_name,
        ])
        .output()
        .with_context(|| format!("start {}", zzcli.display()))?;
    if !output.status.success() {
        let detail = last_non_empty(&output.stderr)
            .or_else(|| last_non_empty(&output.stdout))
            .unwrap_or("zzcli returned non-zero");
        bail!("zzcli failed with status {}: {detail}", output.status);
    }
    let stdout = String::from_utf8(output.stdout).context("decode zzcli output")?;
    let payload = extract_json(&stdout).context("decode zzcli JSON")?;
    if payload.get("respCode").and_then(Value::as_i64).unwrap_or(0) != 0 {
        bail!(
            "ZGateway query failed: {}",
            payload
                .get("errorMsg")
                .and_then(Value::as_str)
                .unwrap_or("unknown response")
        );
    }
    let routes = payload
        .get("respData")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(routes.into_iter().filter_map(normalize_route).collect())
}

fn normalize_route(route: Value) -> Option<HttpRouteKey> {
    let config = route.get("httpToScfFilterConfig").unwrap_or(&Value::Null);
    let scf_path = route
        .get("scfMethodPath")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut path_parts = scf_path.trim_start_matches('/').splitn(2, '/');
    let fallback_interface = path_parts.next().unwrap_or("");
    let fallback_signature = path_parts.next().unwrap_or("");
    let interface_name = config
        .get("interfaceName")
        .and_then(Value::as_str)
        .unwrap_or(fallback_interface)
        .replace("::", ".");
    let signature = config
        .get("methodSignature")
        .and_then(Value::as_str)
        .unwrap_or(fallback_signature);
    let method_name = signature.split('(').next()?.trim();
    if interface_name.is_empty() || method_name.is_empty() {
        return None;
    }
    let method = route.get("httpMethod")?.as_str()?.trim();
    let path = route.get("httpPath")?.as_str()?.trim();
    if method.is_empty() || path.is_empty() {
        return None;
    }
    Some(HttpRouteKey {
        interface_name,
        method_name: method_name.to_owned(),
        signature: Some(signature.to_owned()),
        method: method.to_ascii_uppercase(),
        path: path.to_owned(),
        host: route
            .get("httpHost")
            .and_then(Value::as_str)
            .map(str::to_owned),
        source: RouteSource::Zgateway,
    })
}

fn extract_json(value: &str) -> Result<Value> {
    if let Ok(value) = serde_json::from_str(value.trim()) {
        return Ok(value);
    }
    let start = value.find(['{', '[']).context("zzcli returned no JSON")?;
    let end = value
        .rfind(['}', ']'])
        .context("zzcli returned incomplete JSON")?;
    serde_json::from_str(&value[start..=end]).context("zzcli output is not valid JSON")
}

fn last_non_empty(value: &[u8]) -> Option<&str> {
    let lines = std::str::from_utf8(value).ok()?.lines().collect::<Vec<_>>();
    lines
        .iter()
        .find(|line| line.trim_start().starts_with("Error:"))
        .copied()
        .or_else(|| lines.into_iter().rev().find(|line| !line.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_route_uses_config_identity_and_http_fields() {
        let route = normalize_route(serde_json::json!({
            "httpMethod": "POST",
            "httpPath": "/api/demo/query",
            "httpHost": "demo.test",
            "scfMethodPath": "/fallback/ignored()",
            "httpToScfFilterConfig": {
                "interfaceName": "p.IFacade",
                "methodSignature": "query(QueryReq)"
            }
        }))
        .unwrap();
        assert_eq!(route.interface_name, "p.IFacade");
        assert_eq!(route.method_name, "query");
        assert_eq!(route.path, "/api/demo/query");
        assert!(route.matches("p.IFacade", "query"));
        assert!(!route.matches("other.IFacade", "query"));
        assert!(route.matches_method("p.IFacade", "query", "String (QueryReq request)", true));
        assert!(!route.matches_method("p.IFacade", "query", "String (String request)", true));
        assert!(
            normalize_route(serde_json::json!({
                "httpMethod": "POST",
                "httpPath": "",
                "httpToScfFilterConfig": {
                    "interfaceName": "p.IFacade",
                    "methodSignature": "query(QueryReq)"
                }
            }))
            .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn gateway_query_returns_only_routes_with_http_paths() {
        let zzcli = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zzcli-routes.sh");
        let routes = query_routes(&zzcli, GatewayEnvironment::Testserver, "demo").unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].method_name, "query");
        assert_eq!(routes[0].path, "/api/query");
    }

    #[test]
    fn noisy_output_extracts_one_json_value() {
        assert_eq!(
            extract_json("notice\n{\"respCode\":0,\"respData\":[]}").unwrap()["respCode"],
            0
        );
    }
}
