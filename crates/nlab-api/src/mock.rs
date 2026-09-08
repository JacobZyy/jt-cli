use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

mod scenarios;
mod schema;

#[derive(Clone, Debug, Args)]
pub struct MockArgs {
    /// Generated artifact directory containing OpenAPI
    #[arg(long, value_name = "path")]
    project: PathBuf,
    /// Mock output root inside project
    #[arg(long, default_value = "mock")]
    output_root: String,
    /// Stable global seed
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Optional scenario rules JSON (relative to project, or absolute)
    #[arg(long, value_name = "path")]
    rules: Option<PathBuf>,
    /// Independent manifest path inside project (default: .nlab/mock-manifest.json)
    #[arg(long, value_name = "path")]
    manifest: Option<String>,
    /// Calculate output without writing files
    #[arg(long)]
    dry_run: bool,
    /// Adopt unmanaged or modified mock files
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MockManifest {
    version: u8,
    openapi_sha256: String,
    files: BTreeMap<String, String>,
    rules_file: String,
    rules_sha256: String,
}

pub fn run(args: MockArgs) -> u8 {
    match run_inner(args) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("serialize mock result")
            );
            u8::from(result["failedOperations"].as_u64().unwrap_or(0) > 0)
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

pub(crate) fn automatic(project: &Path, settings: &super::config::MockSettings) -> Result<Value> {
    run_inner(MockArgs {
        project: project.to_owned(),
        output_root: settings.output_root.clone(),
        seed: settings.seed,
        rules: settings.rules.clone(),
        manifest: settings.manifest.clone(),
        dry_run: false,
        force: false,
    })
}

fn run_inner(args: MockArgs) -> Result<Value> {
    validate_relative_root(&args.output_root)?;
    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("resolve generated project {}", args.project.display()))?;
    let config = super::config::ProjectConfig::load(&project)?;
    let openapi_path = pending_or_stable_openapi(&project)?;
    let openapi_source = fs::read_to_string(&openapi_path)?;
    let openapi = serde_json::from_str::<Value>(&openapi_source)?;
    let app_name = openapi["x-nlab"]["appName"]
        .as_str()
        .context("OpenAPI x-nlab.appName missing")?;
    let rules_path = args.rules.as_ref().or(config.mock.rules.as_ref());
    let scenario_rules = match rules_path {
        Some(path) => {
            serde_json::from_str::<scenarios::Rules>(&fs::read_to_string(project.join(path))?)?
        }
        None => scenarios::Rules::default(),
    };
    scenario_rules.validate()?;
    let mut files = BTreeMap::new();
    let mut rules = Vec::new();
    let mut coverage = Vec::new();
    let mut seen = BTreeSet::new();
    for (path, path_item) in openapi["paths"]
        .as_object()
        .context("OpenAPI paths missing")?
    {
        for (method, operation) in path_item.as_object().into_iter().flatten() {
            if !is_http_method(method) {
                continue;
            }
            let key = operation["x-nlab-operation-key"]
                .as_str()
                .context("operation key missing")?;
            seen.insert(key.to_owned());
            let facade = operation["x-nlab-facade"].as_str().unwrap_or("Facade");
            let name = operation["x-nlab-method-name"]
                .as_str()
                .or_else(|| operation["operationId"].as_str())
                .unwrap_or("operation");
            let relative = format!(
                "{}/{}/{}/{}.json",
                args.output_root.trim_end_matches('/'),
                safe_segment(app_name),
                safe_segment(facade),
                safe_segment(name)
            );
            let fallback = scenarios::Operation::default();
            let operation_rules = scenario_rules.operations.get(key).unwrap_or(&fallback);
            let result = generate_operation(
                operation,
                &openapi,
                &scenario_rules,
                operation_rules,
                args.seed,
                key,
            );
            let mut valid_patterns = Vec::new();
            match result {
                Ok((samples, inferred_gaps)) => {
                    let mut artifacts = BTreeMap::new();
                    for (scenario, data) in samples {
                        let filename = if scenario == "default" { relative.clone() }
                            else { format!("{}.{}.json", relative.trim_end_matches(".json"), scenario) };
                        let source = format!("{}\n", serde_json::to_string_pretty(&response_envelope(&config.frontend.response, data))?);
                        if files.insert(filename.clone(), source).is_some() { bail!("mock filename collision: {filename}"); }
                        let selector = if scenario == "default" { None } else { Some(scenario.as_str()) };
                        let pattern = scenarios::selection_pattern(path, &scenario_rules.query, selector)?;
                        // Pattern owns path+query; the single includeFilter owns the method.
                        rules.push(format!("/{pattern}/ file://{} includeFilter://m:/^{}$/ lineProps://important", whistle_file(&project.join(&filename))?, method.to_uppercase()));
                        valid_patterns.push(pattern);
                        artifacts.insert(scenario, filename);
                    }
                    coverage.push(json!({"operation": key, "method": method, "path": path,
                        "generation": if args.dry_run { "planned" } else { "success" },
                        "tier": if operation_rules.tier() == 3 && !inferred_gaps.is_empty() { 2 } else { operation_rules.tier() }, "files": artifacts,
                        "scenarios": operation_rules.scenarios,
                        "sources": operation_rules.sources, "gaps": operation_rules.gaps,
                        "assumptions": operation_rules.assumptions, "inferredGaps": inferred_gaps,
                    }));
                }
                Err(error) => coverage.push(json!({"operation": key, "method": method, "path": path,
                    "generation": "failed", "tier": operation_rules.tier(), "error": format!("{error:#}"),
                    "files": {}, "gaps": operation_rules.gaps, "sources": operation_rules.sources})),
            }
            // Exclusions are OR: every valid selection is excluded from the local error.
            // Unknown/duplicate selectors and failed operations never hit the backend.
            let exclusions = valid_patterns
                .iter()
                .map(|p| format!(" excludeFilter:///{p}/"))
                .collect::<String>();
            rules.push(format!("/{}(?:\\?[^#]*)?$/ statusCode://{} includeFilter://m:/^{}$/ lineProps://important{exclusions}",
                scenarios::path_pattern(path)?, if valid_patterns.is_empty() { 502 } else { 400 }, method.to_uppercase()));
        }
    }
    for key in scenario_rules.operations.keys() {
        if !seen.contains(key) {
            bail!("mock rules operation absent from OpenAPI: {key}");
        }
    }
    let succeeded = coverage
        .iter()
        .filter(|item| item["generation"] != "failed")
        .count();
    let failed = coverage.len() - succeeded;
    let report_file = format!(
        "{}/{}/coverage.json",
        args.output_root.trim_end_matches('/'),
        safe_segment(app_name)
    );
    let report = json!({
        "version": 1, "generator": "jt-nlab-mock/4", "faker": "fake/4.4.0", "seed": args.seed,
        "locale": scenario_rules.locale, "referenceDate": scenario_rules.reference_date,
        "rulesSha256": sha256(&serde_json::to_vec(&scenario_rules)?),
        "openapiSha256": sha256(openapi_source.as_bytes()), "openapiSource": openapi_path,
        "query": scenario_rules.query, "dryRun": args.dry_run,
        "status": if args.dry_run { "planned" } else if failed > 0 { "complete-with-errors" } else { "complete" },
        "assumptions": ["基础样例不证明状态、按钮、金额、时间或标识之间的业务关系；跨接口关联由 base 明确固定。", "图片为商品布局示意素材，不代表实物或质检照片。", "行政区划使用广东省深圳市南山区固定样例，街道门牌和商户组名称为开发示意，不代表实际位置或组织。商品示例使用捷安特 ATX 810 山地自行车及固定开发标识，不代表真实品类库映射。"],
        "operations": coverage,
    });
    files.insert(
        report_file.clone(),
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    );
    let rules_file = format!(
        "{}/{}/whistle.rules",
        args.output_root.trim_end_matches('/'),
        safe_segment(app_name)
    );
    let rules_source = format!(
        "# >>> jt nlab-api {app_name}\n{}\n# <<< jt nlab-api {app_name}\n",
        rules.join("\n").replace('#', "\\x23")
    );
    let manifest_relative = args
        .manifest
        .as_ref()
        .or(config.mock.manifest.as_ref())
        .map(String::as_str)
        .unwrap_or(".nlab/mock-manifest.json");
    if files.contains_key(manifest_relative) || manifest_relative == rules_file {
        bail!("manifest path collides with a generated artifact: {manifest_relative}");
    }
    let manifest_path = safe_target(&project, manifest_relative)?;
    let previous = if manifest_path.exists() {
        let manifest: MockManifest = serde_json::from_str(&fs::read_to_string(&manifest_path)?)
            .with_context(|| format!("invalid mock manifest: {}", manifest_path.display()))?;
        let prefix = format!("{}/", args.output_root.trim_end_matches('/'));
        if !manifest.rules_file.starts_with(&prefix)
            || manifest.files.keys().any(|path| !path.starts_with(&prefix))
        {
            bail!(
                "mock manifest {} belongs to another output root; choose --manifest <unused-relative-path> for isolated output",
                manifest_path.display()
            );
        }
        manifest
    } else {
        MockManifest::default()
    };
    let manifest = MockManifest {
        version: 2,
        openapi_sha256: sha256(openapi_source.as_bytes()),
        files: files
            .iter()
            .map(|(path, source)| (path.clone(), sha256(source.as_bytes())))
            .collect(),
        rules_file: rules_file.clone(),
        rules_sha256: sha256(rules_source.as_bytes()),
    };
    preflight(&project, &files, &rules_file, &previous, args.force)?;
    preflight_stale(&project, &previous, &manifest, args.force)?;
    if !args.dry_run {
        for (relative, source) in files.iter().filter(|(path, _)| **path != report_file) {
            atomic_write(&safe_target(&project, relative)?, source)?;
        }
        atomic_write(&safe_target(&project, &rules_file)?, &rules_source)?;
        remove_stale(&project, &previous, &manifest, args.force)?;
        // Publish successful coverage only after its fixtures and rules exist.
        atomic_write(&safe_target(&project, &report_file)?, &files[&report_file])?;
        atomic_write(
            &manifest_path,
            &format!("{}\n", serde_json::to_string_pretty(&manifest)?),
        )?;
    }
    Ok(json!({
        "status": if args.dry_run { "planned" } else if failed > 0 { "complete-with-errors" } else { "complete" },
        "operations": if args.dry_run { 0 } else { succeeded }, "plannedOperations": succeeded,
        "failedOperations": failed, "coverageFile": report_file,
        "rules": rules.len(), "rulesFile": rules_file, "manifest": manifest_path,
        "dryRun": args.dry_run, "force": args.force,
        "envelope": {"codeField": config.frontend.response.mock_code_field, "dataField": config.frontend.response.mock_data_field}
    }))
}

fn generate_operation(
    operation: &Value,
    document: &Value,
    rules: &scenarios::Rules,
    operation_rules: &scenarios::Operation,
    seed: u64,
    key: &str,
) -> Result<(BTreeMap<String, Value>, BTreeSet<String>)> {
    let schema = success_schema(operation).context("success response schema missing")?;
    let validator = schema::validator(schema, document)?;
    let mut generator = schema::Generator {
        document,
        rules,
        rng: operation_rng(seed, key),
        gaps: BTreeSet::new(),
        fixed: BTreeSet::new(),
    };
    let mut base = generator.generate(schema)?;
    generator.apply_generators(&mut base, &operation_rules.generators)?;
    scenarios::apply_values(&mut base, &operation_rules.base)?;
    let fixed: BTreeSet<String> = operation_rules
        .base
        .keys()
        .chain(operation_rules.generators.keys())
        .chain(generator.fixed.iter())
        .cloned()
        .collect();
    schema::align_pages(&mut base, "", &fixed)?;
    let mut samples = BTreeMap::from([("base".to_owned(), base.clone())]);
    for (name, scenario) in &operation_rules.scenarios {
        let mut sample = base.clone();
        scenarios::apply_values(&mut sample, &scenario.values)?;
        schema::align_pages(
            &mut sample,
            "",
            &fixed
                .iter()
                .chain(scenario.values.keys())
                .cloned()
                .collect(),
        )?;
        samples.insert(name.clone(), sample);
    }
    for (name, sample) in &samples {
        if let Err(error) = validator.validate(sample) {
            bail!("{name}{}: {error}", error.instance_path);
        }
    }
    generator.gaps.retain(|gap| {
        let pointer = gap.split(':').next().unwrap_or("");
        let covered = |values: &BTreeMap<String, Value>| {
            values
                .keys()
                .any(|p| pointer == p || pointer.starts_with(&format!("{p}/")))
        };
        !covered(&operation_rules.base)
            && (operation_rules.scenarios.is_empty()
                || !operation_rules
                    .scenarios
                    .values()
                    .all(|s| covered(&s.values)))
    });
    let default = operation_rules
        .default_scenario
        .as_deref()
        .unwrap_or("base");
    samples.insert("default".to_owned(), samples[default].clone());
    Ok((samples, generator.gaps))
}

fn whistle_file(path: &Path) -> Result<String> {
    let path = path.to_str().context("Whistle file path is not UTF-8")?;
    if path
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '#' | '<' | '>'))
    {
        bail!("Whistle file path contains unsupported whitespace or delimiters: {path}");
    }
    Ok(path.to_owned())
}

fn response_envelope(config: &super::config::ResponseEnvelope, data: Value) -> Value {
    let code = config
        .success_code
        .parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::String(config.success_code.clone()));
    let mut response = Map::new();
    response.insert(config.mock_code_field.clone(), code);
    response.insert(config.mock_data_field.clone(), data);
    Value::Object(response)
}

fn success_schema(operation: &Value) -> Option<&Value> {
    let responses = operation.get("responses")?.as_object()?;
    for status in ["200", "201", "202", "204"] {
        if let Some(schema) = responses
            .get(status)
            .and_then(|response| response.pointer("/content/application~1json/schema"))
        {
            return Some(schema);
        }
    }
    None
}

fn preflight(
    project: &Path,
    files: &BTreeMap<String, String>,
    rules_file: &str,
    previous: &MockManifest,
    force: bool,
) -> Result<()> {
    for relative in files.keys() {
        let target = safe_target(project, relative)?;
        if !target.exists() || force {
            continue;
        }
        let expected = previous
            .files
            .get(relative)
            .map(String::as_str)
            .unwrap_or("");
        let current = fs::read(&target)?;
        if expected.is_empty() || sha256(&current) != expected {
            bail!(
                "refuse to overwrite unmanaged or modified mock file: {}",
                target.display()
            );
        }
    }
    let rules_target = safe_target(project, rules_file)?;
    if rules_target.exists() && !force {
        let current = fs::read(&rules_target)?;
        if previous.rules_sha256.is_empty() || sha256(&current) != previous.rules_sha256 {
            bail!(
                "refuse to overwrite unmanaged or modified rules file: {}",
                rules_target.display()
            );
        }
    }
    Ok(())
}

fn remove_stale(
    project: &Path,
    previous: &MockManifest,
    current: &MockManifest,
    force: bool,
) -> Result<()> {
    for (relative, hash) in &previous.files {
        if current.files.contains_key(relative) {
            continue;
        }
        let target = safe_target(project, relative)?;
        if !target.is_file() {
            continue;
        }
        if !force && sha256(&fs::read(&target)?) != *hash {
            bail!("refuse to remove modified mock file: {}", target.display());
        }
        fs::remove_file(target)?;
    }
    Ok(())
}

fn preflight_stale(
    project: &Path,
    previous: &MockManifest,
    current: &MockManifest,
    force: bool,
) -> Result<()> {
    for (relative, hash) in &previous.files {
        if current.files.contains_key(relative) {
            continue;
        }
        let target = safe_target(project, relative)?;
        if !target.is_file() {
            continue;
        }
        if !force && sha256(&fs::read(&target)?) != *hash {
            bail!("refuse to remove modified mock file: {}", target.display());
        }
    }
    Ok(())
}

fn pending_or_stable_openapi(project: &Path) -> Result<PathBuf> {
    for relative in [
        ".nlab/openapi.pending.json",
        ".nlab/openapi.json",
        "openapi.json",
    ] {
        let path = project.join(relative);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("OpenAPI file not found in {}", project.display())
}

fn operation_rng(seed: u64, operation_key: &str) -> ChaCha8Rng {
    let mut digest = Sha256::new();
    digest.update(seed.to_le_bytes());
    digest.update(operation_key.as_bytes());
    ChaCha8Rng::from_seed(digest.finalize().into())
}

fn safe_target(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe mock path: {relative}");
    }
    let mut target = root.to_owned();
    for component in path.components() {
        let Component::Normal(component) = component else {
            unreachable!();
        };
        target.push(component);
        if fs::symlink_metadata(&target)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("mock path crosses symlink: {}", target.display());
        }
    }
    Ok(target)
}

fn validate_relative_root(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("--output-root must be a safe relative directory");
    }
    Ok(())
}

fn atomic_write(path: &Path, source: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().context("mock target has no parent")?)?;
    temporary.write_all(source.as_bytes())?;
    temporary.persist(path)?;
    Ok(())
}

fn safe_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn sha256(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_http_method(value: &str) -> bool {
    matches!(value, "get" | "post" | "put" | "patch" | "delete")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn operation_seed_is_repeatable_and_operation_scoped() {
        let mut first = operation_rng(42, "Facade#a");
        let mut second = operation_rng(42, "Facade#a");
        let mut other = operation_rng(42, "Facade#b");
        assert_eq!(first.random::<u64>(), second.random::<u64>());
        assert_ne!(first.random::<u64>(), other.random::<u64>());
    }

    #[test]
    fn response_envelope_uses_project_fields() {
        let config = serde_json::from_value::<super::super::config::ResponseEnvelope>(json!({
            "successCode": "0",
            "codeFields": ["code", "respCode"],
            "dataFields": ["data", "respData"],
            "mockCodeField": "respCode",
            "mockDataField": "respData"
        }))
        .unwrap();
        assert_eq!(
            response_envelope(&config, json!({"id": 1})),
            json!({"respCode": 0, "respData": {"id": 1}})
        );
    }
}

#[cfg(test)]
#[path = "mock/tests.rs"]
mod integration_tests;
