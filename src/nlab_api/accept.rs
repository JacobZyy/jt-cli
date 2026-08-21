use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Args)]
pub struct AcceptArgs {
    /// Generated artifact directory
    #[arg(long, value_name = "path")]
    project: PathBuf,
    /// Backend repository used to verify branch and commit identity
    #[arg(long, value_name = "path")]
    repo_path: PathBuf,
    /// Explicit Mock decision
    #[arg(long, value_enum)]
    mock: MockDecision,
    /// Accept without a completed replacement map
    #[arg(long)]
    skip_migration: bool,
    /// Accept unresolved placeholder routes
    #[arg(long)]
    allow_placeholder_routes: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum MockDecision {
    Generated,
    Skipped,
}

impl MockDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Generated => "generated",
            Self::Skipped => "skipped",
        }
    }
}

pub fn run(args: AcceptArgs) -> u8 {
    match run_inner(args) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("serialize accept result")
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

fn run_inner(args: AcceptArgs) -> Result<Value> {
    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("resolve generated project {}", args.project.display()))?;
    let ir_path = project.join(".nlab/contract-ir.json");
    let ir = serde_json::from_str::<super::model::ContractIr>(&fs::read_to_string(&ir_path)?)?;
    let backend = super::repo::inspect(&args.repo_path, &ir.target.branch)?;
    if backend.commit != ir.target.commit {
        bail!(
            "backend commit changed since generation: {} -> {}",
            ir.target.commit,
            backend.commit
        );
    }
    let pending = project.join(".nlab/openapi.pending.json");
    let source = fs::read_to_string(&pending)
        .with_context(|| format!("pending OpenAPI missing: {}", pending.display()))?;
    let openapi = serde_json::from_str::<Value>(&source)?;
    let pending_sha256 = sha256(source.as_bytes());
    super::openapi::validate(&openapi, ir.operations.len())?;
    let placeholders = openapi["x-nlab-contracts"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, operation)| operation["x-nlab-route-status"] == "placeholder")
        .count();
    if placeholders > 0 && !args.allow_placeholder_routes {
        bail!(
            "{placeholders} placeholder routes remain; resolve routes or pass --allow-placeholder-routes"
        );
    }
    if !args.skip_migration {
        validate_replacement_map(&project.join(".nlab/replacement-map.json"), &pending_sha256)?;
    }
    if args.mock == MockDecision::Generated {
        validate_mock_manifest(&project.join(".nlab/mock-manifest.json"), &pending_sha256)?;
    }
    let stable = project.join(".nlab/openapi.json");
    atomic_write(&stable, &source)?;
    fs::remove_file(&pending)?;
    let acceptance = serde_json::json!({
        "version": 1,
        "branch": ir.target.branch,
        "commit": ir.target.commit,
        "openapiSha256": pending_sha256,
        "placeholderRoutes": placeholders,
        "migration": if args.skip_migration { "skipped" } else { "complete" },
        "mock": args.mock.as_str()
    });
    let acceptance_path = project.join(".nlab/acceptance.json");
    atomic_write(
        &acceptance_path,
        &format!("{}\n", serde_json::to_string_pretty(&acceptance)?),
    )?;
    Ok(serde_json::json!({
        "status": "complete",
        "openapi": stable,
        "acceptance": acceptance_path,
        "placeholderRoutes": placeholders,
        "migration": acceptance["migration"],
        "mock": acceptance["mock"]
    }))
}

fn validate_replacement_map(path: &Path, openapi_sha256: &str) -> Result<()> {
    let value = serde_json::from_str::<Value>(
        &fs::read_to_string(path)
            .with_context(|| format!("replacement map missing: {}", path.display()))?,
    )?;
    let unresolved = value["unresolved"].as_array().map_or(0, Vec::len);
    if unresolved > 0 {
        bail!("replacement map has {unresolved} unresolved decisions");
    }
    if value["newOpenapiSha256"].as_str() != Some(openapi_sha256) {
        bail!("replacement map does not match pending OpenAPI");
    }
    Ok(())
}

fn validate_mock_manifest(path: &Path, openapi_sha256: &str) -> Result<()> {
    let value = serde_json::from_str::<Value>(
        &fs::read_to_string(path)
            .with_context(|| format!("Mock manifest missing: {}", path.display()))?,
    )?;
    if value["openapiSha256"].as_str() != Some(openapi_sha256) {
        bail!("Mock manifest does not match pending OpenAPI");
    }
    Ok(())
}

fn atomic_write(path: &Path, source: &str) -> Result<()> {
    if fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!("refuse to replace symlink: {}", path.display());
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, source)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn sha256(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_map_rejects_unresolved_decisions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("replacement.json");
        fs::write(
            &path,
            r#"{"newOpenapiSha256":"hash","unresolved":["ambiguous"]}"#,
        )
        .unwrap();
        assert!(validate_replacement_map(&path, "hash").is_err());
        fs::write(&path, r#"{"newOpenapiSha256":"hash","unresolved":[]}"#).unwrap();
        assert!(validate_replacement_map(&path, "hash").is_ok());
        assert!(validate_replacement_map(&path, "other").is_err());

        fs::write(&path, r#"{"openapiSha256":"hash"}"#).unwrap();
        assert!(validate_mock_manifest(&path, "hash").is_ok());
        assert!(validate_mock_manifest(&path, "other").is_err());
    }
}
