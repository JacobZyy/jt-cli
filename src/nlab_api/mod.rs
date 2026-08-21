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
use std::io::{IsTerminal, stderr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;

use graph::Snapshot;
use java::JavaProject;
use model::{ContractIr, GenerateResult, ProvenanceStatus, TargetIdentity};
use output::OutputLock;
use semantic::SemanticAnalyzer;

const MAX_TIMEOUT_SECONDS: u64 = 20 * 60;
const PHASES: u64 = 7;

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

    reporter.phase("inspect backend repository");
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
    reporter.advance();

    reporter.phase("initialize CodeGraph index");
    repo::sync_codegraph(&target, deadline)?;
    reporter.advance();

    reporter.phase("load CodeGraph snapshot");
    ensure_before_deadline(deadline)?;
    let graph = Snapshot::load(&target.root)?;
    reporter.advance();

    reporter.phase("build Facade and DTO contract graph");
    ensure_before_deadline(deadline)?;
    let project = JavaProject::load(&target.root, &graph)?;
    let identity = TargetIdentity {
        app_name: config.backend.app_name.clone(),
        branch: target.branch.clone(),
        commit: target.commit.clone(),
        codegraph_version: graph.version.clone(),
        codegraph_extraction_version: graph.extraction_version.clone(),
    };
    let (mut operations, mut schemas) = project.build_contracts(&identity)?;
    if operations.is_empty() {
        bail!("no @ServiceContract Facade operations found");
    }
    reporter.advance();

    reporter.phase("analyze response field provenance");
    ensure_before_deadline(deadline)?;
    let mut semantic = SemanticAnalyzer::new(&project);
    semantic.enrich_linked_enums(&mut schemas)?;
    semantic.enrich(&mut operations, &schemas)?;
    let ir = ContractIr {
        target: identity,
        operations,
        schemas,
    };
    reporter.advance();

    reporter.phase("generate OpenAPI and TypeScript");
    ensure_before_deadline(deadline)?;
    let openapi = openapi::generate(&ir, &config)?;
    let frontend = typescript::generate(&ir, &config)?;
    reporter.advance();

    reporter.phase("write and verify artifacts");
    ensure_before_deadline(deadline)?;
    let written = output::write(&output_dir, &ir, &openapi, &frontend)?;
    repo::verify_unchanged(&target)?;
    reporter.advance();
    reporter.complete("nlab-api artifacts generated");

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
    Ok(GenerateResult {
        status: "complete",
        repo_path: target.root.display().to_string(),
        branch: target.branch,
        commit: target.commit,
        output_dir: output_dir.display().to_string(),
        openapi: written.openapi_path.display().to_string(),
        openapi_sha256: written.openapi_sha256,
        contracts: ir.operations.len(),
        paths: openapi.paths,
        schemas: openapi.schemas,
        placeholders: ir.operations.len(),
        semantic_patches,
        closed_enum_patches,
        api_files: written.api_files.len(),
        type_files: written.type_files.len(),
        enum_files: written.enum_files.len(),
        mock_generated: false,
        whistle_rules_updated: false,
        duration_ms: started.elapsed().as_millis(),
    })
}

struct PhaseReporter {
    progress: Option<cliclack::ProgressBar>,
}

impl PhaseReporter {
    fn new() -> Self {
        let progress = stderr().is_terminal().then(|| {
            let progress = cliclack::progress_bar(PHASES);
            progress.start("nlab-api generate");
            progress
        });
        Self { progress }
    }

    fn phase(&self, phase: &str) {
        if let Some(progress) = &self.progress {
            progress.set_message(phase);
        } else {
            eprintln!(
                "[nlab-api] {}",
                serde_json::json!({ "phase": phase, "status": "start" })
            );
        }
    }

    fn advance(&self) {
        if let Some(progress) = &self.progress {
            progress.inc(1);
        }
    }

    fn complete(&self, message: &str) {
        if let Some(progress) = &self.progress {
            progress.stop(message);
        } else {
            eprintln!(
                "[nlab-api] {}",
                serde_json::json!({ "status": "complete", "message": message })
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
