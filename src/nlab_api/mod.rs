mod accept;
mod coded_values;
mod config;
mod graph;
mod init;
mod java;
mod layout;
mod migrate;
mod mock;
mod model;
mod naming;
mod openapi;
mod output;
mod repo;
mod routes;
mod semantic;
mod typescript;

use std::env;
use std::fs;
use std::io::{IsTerminal, stderr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::{Value, json};

use graph::Snapshot;
use java::JavaProject;
use model::{ContractIr, GenerateResult, ProvenanceStatus, RouteStatus, TargetIdentity};
use output::OutputLock;
use semantic::SemanticAnalyzer;

const MAX_TIMEOUT_SECONDS: u64 = 20 * 60;

#[derive(Clone, Debug, Args)]
pub struct GenerateArgs {
    /// Frontend project containing .nlab/nlab-api.config.json
    #[arg(long, value_name = "path", default_value = ".")]
    project: PathBuf,
    /// Overall deadline, capped at 1200 seconds
    #[arg(long, default_value_t = MAX_TIMEOUT_SECONDS)]
    timeout_seconds: u64,
}

pub use accept::AcceptArgs;
pub use init::InitArgs;
pub use migrate::MigrateArgs;
pub use mock::MockArgs;
pub use routes::RoutesArgs;

pub fn routes(args: RoutesArgs) -> u8 {
    routes::run(args)
}

pub fn migrate(args: MigrateArgs) -> u8 {
    migrate::run(args)
}

pub fn mock(args: MockArgs) -> u8 {
    mock::run(args)
}

pub fn accept(args: AcceptArgs) -> u8 {
    accept::run(args)
}

pub fn init(args: InitArgs) -> u8 {
    init::run(args)
}

pub fn generate(args: GenerateArgs) -> u8 {
    match generate_inner(args) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("serialize generation result")
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            if error
                .chain()
                .any(|cause| cause.to_string().contains("deadline reached"))
            {
                124
            } else {
                1
            }
        }
    }
}

fn generate_inner(args: GenerateArgs) -> Result<GenerateResult> {
    if args.timeout_seconds == 0 || args.timeout_seconds > MAX_TIMEOUT_SECONDS {
        bail!("--timeout-seconds must be between 1 and {MAX_TIMEOUT_SECONDS}");
    }
    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.timeout_seconds);
    let reporter = PhaseReporter::new();

    reporter.phase(0, "读取配置");
    let output_dir = absolute_path(&args.project)?
        .canonicalize()
        .with_context(|| format!("resolve frontend project {}", args.project.display()))?;
    let config = config::ProjectConfig::load(&output_dir)?;
    config.validate_project(&output_dir)?;
    let target = repo::inspect(&config.backend.repo_path, &config.backend.branch)?;
    if path_inside(&target.root, &output_dir) {
        bail!("frontend project must stay outside backend repository");
    }
    let _lock = OutputLock::acquire(&output_dir)?;
    let legacy = if config.migration.enabled {
        migrate::snapshot_legacy(&output_dir)?
    } else {
        None
    };

    reporter.phase(10, "同步 CodeGraph");
    repo::sync_codegraph(&target, deadline)?;

    reporter.phase(25, "读取后端索引");
    ensure_before_deadline(deadline)?;
    let graph = Snapshot::load(&target.root)?;

    reporter.phase(35, "解析后端契约");
    ensure_before_deadline(deadline)?;
    let project = JavaProject::load(&target.root, &graph)?;
    let identity = TargetIdentity {
        app_name: config.backend.app_name.clone(),
        branch: target.branch.clone(),
        commit: target.commit.clone(),
        codegraph_version: graph.version.clone(),
        codegraph_extraction_version: graph.extraction_version.clone(),
    };
    let (mut operations, mut schemas) =
        project.build_contracts(&identity, &config.backend.contract_roots)?;
    if operations.is_empty() {
        bail!("no @ServiceContract Facade operations found");
    }

    reporter.phase(50, "分析枚举与注释");
    ensure_before_deadline(deadline)?;
    let mut semantic = SemanticAnalyzer::new(&project);
    semantic.enrich_linked_enums(&mut schemas)?;
    semantic.enrich(&mut operations, &schemas)?;
    let mut ir = ContractIr {
        target: identity,
        operations,
        schemas,
    };
    let mut diagnostics = semantic_diagnostics(&ir);

    reporter.phase(60, "补全 Gateway 路由");
    let route_summary = if config.gateway.enabled {
        routes::apply_best_effort(&mut ir, Path::new("zzcli"))
    } else {
        routes::RouteSummary {
            replaced: 0,
            placeholders: ir.operations.len(),
            missing: Vec::new(),
            warning: None,
        }
    };
    if let Some(warning) = &route_summary.warning {
        diagnostics.push(json!({
            "level": "warning",
            "stage": "routes",
            "code": "GATEWAY_QUERY_FAILED",
            "message": warning,
            "fallback": "placeholder"
        }));
    } else {
        diagnostics.extend(route_summary.missing.iter().map(|operation_key| {
            json!({
                "level": "warning",
                "stage": "routes",
                "code": "GATEWAY_ROUTE_NOT_FOUND",
                "operationKey": operation_key,
                "fallback": "placeholder"
            })
        }));
    }

    reporter.phase(70, "生成前端代码");
    ensure_before_deadline(deadline)?;
    let openapi = openapi::generate(&ir, &config)?;
    let frontend = typescript::generate(&ir, &config)?;

    reporter.phase(78, "写入生成产物");
    ensure_before_deadline(deadline)?;
    let written = output::write(&output_dir, &ir, &openapi, &frontend)?;

    reporter.phase(85, "迁移业务引用");
    let migration = if let Some(legacy) = &legacy {
        migrate::automatic(
            &output_dir,
            legacy.path(),
            &output_dir.join(&config.frontend.source_root),
        )?
    } else {
        json!({
            "status": "skipped",
            "reason": if config.migration.enabled { "no-legacy-state" } else { "disabled" },
            "changedSourceFiles": [],
            "unresolvedCount": 0
        })
    };
    let migration_changed_source_files = migration["changedSourceFiles"]
        .as_array()
        .map_or(0, Vec::len);
    let migration_unresolved = migration["unresolvedCount"].as_u64().unwrap_or(0) as usize;
    diagnostics.extend(migration_diagnostics(&output_dir)?);

    reporter.phase(93, "生成可选 Mock");
    let mock_result = if config.mock.enabled {
        Some(mock::automatic(&output_dir, &config.mock)?)
    } else {
        None
    };
    let mock_generated = mock_result
        .as_ref()
        .and_then(|value| value["operations"].as_u64())
        .is_some_and(|count| count > 0);
    let whistle_rules_updated = mock_result
        .as_ref()
        .and_then(|value| value["rules"].as_u64())
        .is_some_and(|count| count > 0);

    reporter.phase(98, "完成生成");
    repo::verify_unchanged(&target)?;
    let stable_openapi = output::promote_openapi(&output_dir)?;

    let semantic_patches = ir
        .operations
        .iter()
        .map(|operation| operation.semantic_patches.len())
        .sum();
    let closed_enum_patches = ir
        .operations
        .iter()
        .flat_map(|operation| &operation.semantic_patches)
        .filter(|patch| patch.status == ProvenanceStatus::Closed)
        .count();
    let placeholders = ir
        .operations
        .iter()
        .filter(|operation| operation.route.status == RouteStatus::Placeholder)
        .count();
    let status = if diagnostics.is_empty() {
        "complete"
    } else {
        "complete-with-warnings"
    };
    let warning_count = diagnostics.len();
    let report = json!({
        "version": 1,
        "status": status,
        "backend": {
            "repoPath": target.root,
            "branch": target.branch,
            "commit": target.commit,
        },
        "stages": {
            "generate": "complete",
            "routes": {
                "status": if route_summary.warning.is_some() || route_summary.placeholders > 0 { "partial" } else { "complete" },
                "replaced": route_summary.replaced,
                "placeholder": route_summary.placeholders,
            },
            "migration": migration,
            "mock": mock_result.as_ref().map_or_else(
                || json!({ "status": "skipped", "enabled": false }),
                |value| value.clone()
            ),
        },
        "diagnostics": diagnostics,
        "artifacts": {
            "openapi": stable_openapi,
            "apiFiles": written.api_files,
            "typeFiles": written.type_files,
            "enumFiles": written.enum_files,
        }
    });
    let report_path = output::write_report(&output_dir, &report)?;
    reporter.complete("nlab-api generate 完成");

    Ok(GenerateResult {
        status,
        repo_path: target.root.display().to_string(),
        branch: target.branch,
        commit: target.commit,
        output_dir: output_dir.display().to_string(),
        openapi: stable_openapi.display().to_string(),
        openapi_sha256: written.openapi_sha256,
        contracts: ir.operations.len(),
        paths: openapi.paths,
        schemas: openapi.schemas,
        routes_replaced: route_summary.replaced,
        placeholders,
        semantic_patches,
        closed_enum_patches,
        api_files: written.api_files.len(),
        type_files: written.type_files.len(),
        enum_files: written.enum_files.len(),
        migration_changed_source_files,
        migration_unresolved,
        mock_generated,
        whistle_rules_updated,
        warnings: warning_count,
        report: report_path.display().to_string(),
        duration_ms: started.elapsed().as_millis(),
    })
}

fn semantic_diagnostics(ir: &ContractIr) -> Vec<Value> {
    ir.operations
        .iter()
        .flat_map(|operation| {
            operation
                .semantic_patches
                .iter()
                .filter(|patch| {
                    matches!(
                        patch.status,
                        ProvenanceStatus::Known | ProvenanceStatus::External
                    )
                })
                .map(|patch| {
                    json!({
                        "level": "info",
                        "stage": "generate",
                        "code": format!("ENUM_{:?}", patch.status).to_ascii_uppercase(),
                        "operationKey": operation.key,
                        "schemaFqn": patch.target.schema_fqn,
                        "fieldPath": patch.target.field_path,
                        "message": patch.warning,
                    })
                })
        })
        .collect()
}

fn migration_diagnostics(project: &Path) -> Result<Vec<Value>> {
    let path = project.join(".nlab/replacement-map.json");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    Ok(value["unresolved"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|message| {
            json!({
                "level": "warning",
                "stage": "migration",
                "code": "MIGRATION_REFERENCE_RETAINED",
                "message": message,
            })
        })
        .collect())
}

struct PhaseReporter {
    progress: Option<cliclack::ProgressBar>,
}

impl PhaseReporter {
    fn new() -> Self {
        let progress = stderr().is_terminal().then(|| {
            let progress = cliclack::progress_bar(100);
            progress.start("nlab-api generate");
            progress
        });
        Self { progress }
    }

    fn phase(&self, percent: u64, phase: &str) {
        if let Some(progress) = &self.progress {
            progress.set_position(percent);
            progress.set_message(phase);
        } else {
            eprintln!(
                "{}",
                serde_json::json!({ "type": "progress", "stage": phase, "percent": percent })
            );
        }
    }

    fn complete(&self, message: &str) {
        if let Some(progress) = &self.progress {
            progress.set_position(100);
            progress.stop(message);
        } else {
            eprintln!(
                "{}",
                serde_json::json!({ "type": "progress", "stage": message, "percent": 100 })
            );
        }
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn path_inside(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

fn ensure_before_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        bail!("generation deadline reached");
    }
    Ok(())
}
