use semver::Version;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const WORKFLOW_PATH: &str = ".github/workflows/npm-release.yml";
const RELEASE_CONFIG_PATH: &str = "release-please-config.json";
const RELEASE_MANIFEST_PATH: &str = ".release-please-manifest.json";

const WORKFLOW: &str = r#"name: package release

on:
  push:

permissions:
  contents: write
  issues: write
  pull-requests: write
  id-token: write

jobs:
  release:
    if: github.ref_name == github.event.repository.default_branch
    uses: JacobZyy/jt-cli/.github/workflows/npm-release.yml@v1.4.0
"#;

#[derive(Debug, Eq, PartialEq)]
pub enum InitStatus {
    Created(Vec<PathBuf>),
    Unchanged(Vec<PathBuf>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageManager {
    Npm,
    Pnpm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReleaseType {
    Node,
    Rust,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleasePackage {
    path: String,
    version: String,
    release_type: ReleaseType,
}

#[derive(Debug)]
struct Project {
    packages: Vec<ReleasePackage>,
}

pub fn init(root: &Path) -> Result<InitStatus, String> {
    let project = inspect_project(root)?;
    let workflow_path = root.join(WORKFLOW_PATH);
    let config_path = root.join(RELEASE_CONFIG_PATH);
    let manifest_path = root.join(RELEASE_MANIFEST_PATH);

    if workflow_path.exists() {
        let current = fs::read_to_string(&workflow_path)
            .map_err(|error| format!("cannot read {}: {error}", workflow_path.display()))?;
        if current != WORKFLOW {
            return Err(format!(
                "{} already exists; refusing to overwrite it",
                workflow_path.display()
            ));
        }
    }

    let config = if config_path.exists() {
        None
    } else {
        Some(project.release_config()?)
    };
    let manifest = if manifest_path.exists() {
        None
    } else {
        Some(project.release_manifest()?)
    };
    let files = [
        (config_path.clone(), config),
        (manifest_path.clone(), manifest),
        (
            workflow_path.clone(),
            (!workflow_path.exists()).then(|| WORKFLOW.to_owned()),
        ),
    ];

    let mut created = Vec::new();
    for (path, content) in files {
        let Some(content) = content else {
            continue;
        };
        if let Err(error) = create_file(&path, content.as_bytes()) {
            for created_path in created.iter().rev() {
                let _ = fs::remove_file(created_path);
            }
            return Err(error);
        }
        created.push(path);
    }

    if created.is_empty() {
        Ok(InitStatus::Unchanged(vec![
            workflow_path,
            config_path,
            manifest_path,
        ]))
    } else {
        Ok(InitStatus::Created(created))
    }
}

fn create_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid file path: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    Ok(())
}

fn inspect_project(root: &Path) -> Result<Project, String> {
    let origin = git_origin(root)?;
    if is_gitlab(&origin) {
        return Err("GitLab release initialization is not supported yet".to_owned());
    }
    let origin_repository =
        github_slug(&origin).ok_or_else(|| "Git origin must point to github.com".to_owned())?;

    let has_node = root.join("package.json").is_file();
    let has_rust = root.join("Cargo.toml").is_file();
    if !has_node && !has_rust {
        return Err(
            "unsupported project; jt repo cicd currently supports Node.js and Rust".to_owned(),
        );
    }

    let mut packages = Vec::new();
    if has_node {
        packages.extend(inspect_node_packages(root, &origin_repository)?);
    }
    if has_rust {
        packages.extend(inspect_rust_packages(root, &origin_repository)?);
    }
    if packages.is_empty() {
        return Err("project has no publishable Node.js packages or Rust crates".to_owned());
    }
    packages.sort_by(|left, right| {
        left.path.cmp(&right.path).then_with(|| {
            release_type_name(left.release_type).cmp(release_type_name(right.release_type))
        })
    });

    Ok(Project { packages })
}

fn inspect_node_packages(
    root: &Path,
    origin_repository: &str,
) -> Result<Vec<ReleasePackage>, String> {
    let root_path = root.join("package.json");
    let root_package = read_json_object(&root_path)?;
    detect_package_manager(root, &root_package)?;

    let monorepo = root.join("turbo.json").is_file()
        || root.join("pnpm-workspace.yaml").is_file()
        || root_package.contains_key("workspaces");
    let package_paths = if monorepo {
        find_package_json_files(root)?
    } else {
        vec![root_path]
    };

    let mut packages = Vec::new();
    for package_path in package_paths {
        let package = read_json_object(&package_path)?;
        match package.get("private") {
            Some(Value::Bool(true)) => continue,
            Some(Value::Bool(false)) | None => {}
            Some(_) => {
                return Err(format!(
                    "{} private must be a boolean",
                    package_path.display()
                ));
            }
        }

        required_string(&package, "name", &package_path)?;
        let version = required_string(&package, "version", &package_path)?;
        Version::parse(version)
            .map_err(|error| format!("{} version is invalid: {error}", package_path.display()))?;
        validate_registry(&package, &package_path)?;
        validate_repository(
            package_repository(&package, &package_path)?,
            origin_repository,
            &package_path,
        )?;

        packages.push(ReleasePackage {
            path: relative_package_path(root, &package_path)?,
            version: version.to_owned(),
            release_type: ReleaseType::Node,
        });
    }

    Ok(packages)
}

fn find_package_json_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    const IGNORED_DIRECTORIES: [&str; 7] = [
        ".git",
        ".turbo",
        "node_modules",
        "target",
        "dist",
        "build",
        "coverage",
    ];

    fn walk(directory: &Path, result: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = fs::read_dir(directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_file() && entry.file_name() == "package.json" {
                result.push(path);
                continue;
            }
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || IGNORED_DIRECTORIES.contains(&name.as_ref()) {
                continue;
            }
            walk(&path, result)?;
        }
        Ok(())
    }

    let mut result = Vec::new();
    walk(root, &mut result)?;
    result.sort();
    Ok(result)
}

fn inspect_rust_packages(
    root: &Path,
    origin_repository: &str,
) -> Result<Vec<ReleasePackage>, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            "cargo metadata failed".to_owned()
        } else {
            format!("cargo metadata failed: {detail}")
        });
    }

    let metadata = serde_json::from_slice::<Value>(&output.stdout)
        .map_err(|error| format!("cargo metadata returned invalid JSON: {error}"))?;
    let workspace_members = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata omitted workspace_members".to_owned())?
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let metadata_packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata omitted packages".to_owned())?;

    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve current directory: {error}"))?;
    let mut packages = Vec::new();
    for package in metadata_packages {
        let Some(package) = package.as_object() else {
            return Err("cargo metadata package must be an object".to_owned());
        };
        let id = package
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "cargo metadata package omitted id".to_owned())?;
        if !workspace_members.contains(id) || !rust_package_is_publishable(package)? {
            continue;
        }

        let version = package
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| "cargo metadata package omitted version".to_owned())?;
        Version::parse(version)
            .map_err(|error| format!("Cargo package version is invalid: {error}"))?;
        let repository = package
            .get("repository")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "publishable Cargo package repository is required".to_owned())?;
        let manifest_path = package
            .get("manifest_path")
            .and_then(Value::as_str)
            .ok_or_else(|| "cargo metadata package omitted manifest_path".to_owned())?;
        let manifest_path = fs::canonicalize(manifest_path)
            .map_err(|error| format!("cannot resolve Cargo manifest: {error}"))?;
        validate_repository(repository, origin_repository, &manifest_path)?;
        let package_directory = manifest_path
            .parent()
            .ok_or_else(|| format!("invalid Cargo manifest path: {}", manifest_path.display()))?;
        let relative = package_directory
            .strip_prefix(&canonical_root)
            .map_err(|_| {
                format!(
                    "Cargo workspace package is outside Git repository: {}",
                    package_directory.display()
                )
            })?;

        packages.push(ReleasePackage {
            path: normalize_relative_path(relative),
            version: version.to_owned(),
            release_type: ReleaseType::Rust,
        });
    }

    Ok(packages)
}

fn rust_package_is_publishable(package: &Map<String, Value>) -> Result<bool, String> {
    match package.get("publish") {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Array(registries)) if registries.is_empty() => Ok(false),
        Some(Value::Array(registries))
            if registries
                .iter()
                .all(|registry| registry.as_str() == Some("crates-io")) =>
        {
            Ok(true)
        }
        Some(Value::Array(_)) => Err("only the crates.io Cargo registry is supported".to_owned()),
        Some(_) => Err("cargo metadata publish must be null or an array".to_owned()),
    }
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str::<Value>(&source)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} root must be an object", path.display()))
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &Path,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{} {key} must be a non-empty string", path.display()))
}

fn validate_registry(package: &Map<String, Value>, path: &Path) -> Result<(), String> {
    let Some(publish_config) = package.get("publishConfig") else {
        return Ok(());
    };
    let publish_config = publish_config
        .as_object()
        .ok_or_else(|| format!("{} publishConfig must be an object", path.display()))?;
    let Some(registry) = publish_config.get("registry") else {
        return Ok(());
    };
    let registry = registry
        .as_str()
        .ok_or_else(|| format!("{} publishConfig.registry must be a string", path.display()))?;

    if registry.trim_end_matches('/') != "https://registry.npmjs.org" {
        return Err("only https://registry.npmjs.org is supported".to_owned());
    }
    Ok(())
}

fn detect_package_manager(
    root: &Path,
    package: &Map<String, Value>,
) -> Result<PackageManager, String> {
    if let Some(declared) = package.get("packageManager") {
        let declared = declared
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "package.json packageManager must be a non-empty string".to_owned())?;
        let (manager, version) = declared.split_once('@').ok_or_else(|| {
            "package.json packageManager must include an exact version".to_owned()
        })?;
        Version::parse(version)
            .map_err(|error| format!("packageManager {manager} version is invalid: {error}"))?;
        return match manager {
            "npm" => Ok(PackageManager::Npm),
            "pnpm" => Ok(PackageManager::Pnpm),
            "yarn" | "bun" => Err(format!(
                "Node package manager {manager} is not supported yet; supported: npm, pnpm"
            )),
            _ => Err(format!(
                "package manager {manager} is not supported yet; supported: npm, pnpm"
            )),
        };
    }

    let npm = root.join("package-lock.json").is_file();
    let pnpm = root.join("pnpm-lock.yaml").is_file();
    match (npm, pnpm) {
        (true, false) | (false, false) => {
            if !npm
                && (root.join("yarn.lock").is_file()
                    || root.join("bun.lock").is_file()
                    || root.join("bun.lockb").is_file())
            {
                return Err(
                    "Node package manager is not supported yet; supported: npm, pnpm".to_owned(),
                );
            }
            if !npm && (root.join("deno.json").is_file() || root.join("deno.lock").is_file()) {
                return Err("Deno projects are not supported yet".to_owned());
            }
            Ok(PackageManager::Npm)
        }
        (false, true) => {
            Err("pnpm projects require packageManager, for example pnpm@10.15.0".to_owned())
        }
        (true, true) => Err(
            "multiple Node lockfiles require an explicit packageManager in package.json".to_owned(),
        ),
    }
}

fn package_repository<'a>(package: &'a Map<String, Value>, path: &Path) -> Result<&'a str, String> {
    match package.get("repository") {
        Some(Value::String(repository)) if !repository.trim().is_empty() => Ok(repository),
        Some(Value::Object(repository)) => repository
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "{} repository.url must be a non-empty string",
                    path.display()
                )
            }),
        _ => Err(format!("{} repository is required", path.display())),
    }
}

fn validate_repository(
    repository: &str,
    origin_repository: &str,
    path: &Path,
) -> Result<(), String> {
    let package_repository = github_slug(repository)
        .ok_or_else(|| format!("{} repository must point to github.com", path.display()))?;
    if !origin_repository.eq_ignore_ascii_case(&package_repository) {
        return Err(format!(
            "{} repository {package_repository} does not match Git origin {origin_repository}",
            path.display()
        ));
    }
    Ok(())
}

fn relative_package_path(root: &Path, package_path: &Path) -> Result<String, String> {
    let directory = package_path
        .parent()
        .ok_or_else(|| format!("invalid package path: {}", package_path.display()))?;
    let relative = directory.strip_prefix(root).map_err(|_| {
        format!(
            "package is outside Git repository: {}",
            package_path.display()
        )
    })?;
    Ok(normalize_relative_path(relative))
}

fn normalize_relative_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        path.components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl Project {
    fn release_config(&self) -> Result<String, String> {
        let mut packages = BTreeMap::new();
        for package in &self.packages {
            if packages
                .insert(
                    package.path.clone(),
                    json!({ "release-type": release_type_name(package.release_type) }),
                )
                .is_some()
            {
                return Err(format!(
                    "publishable Node.js and Rust packages cannot share release path {}; provide a custom {RELEASE_CONFIG_PATH}",
                    package.path
                ));
            }
        }

        let node_count = self
            .packages
            .iter()
            .filter(|package| package.release_type == ReleaseType::Node)
            .count();
        let rust_count = self
            .packages
            .iter()
            .filter(|package| package.release_type == ReleaseType::Rust)
            .count();
        let mut plugins = Vec::new();
        if node_count > 1 {
            plugins.push("node-workspace");
        }
        if rust_count > 1 {
            plugins.push("cargo-workspace");
        }

        let config = if plugins.is_empty() {
            json!({ "packages": packages })
        } else {
            json!({ "plugins": plugins, "packages": packages })
        };
        pretty_json(&config)
    }

    fn release_manifest(&self) -> Result<String, String> {
        let mut manifest = BTreeMap::new();
        for package in &self.packages {
            if manifest
                .insert(package.path.clone(), package.version.clone())
                .is_some()
            {
                return Err(format!(
                    "publishable Node.js and Rust packages cannot share release path {}; provide a custom {RELEASE_MANIFEST_PATH}",
                    package.path
                ));
            }
        }
        pretty_json(&json!(manifest))
    }
}

fn pretty_json(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|value| format!("{value}\n"))
        .map_err(|error| format!("cannot generate release configuration: {error}"))
}

fn release_type_name(release_type: ReleaseType) -> &'static str {
    match release_type {
        ReleaseType::Node => "node",
        ReleaseType::Rust => "rust",
    }
}

fn git_origin(root: &Path) -> Result<String, String> {
    let top_level = git_output(root, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "current directory must be a Git repository root".to_owned())?;
    let top_level = fs::canonicalize(top_level)
        .map_err(|error| format!("cannot resolve Git repository root: {error}"))?;
    let root = fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve current directory: {error}"))?;

    if root != top_level {
        return Err("current directory must be the Git repository root".to_owned());
    }

    git_output(&root, &["remote", "get-url", "origin"])
        .map_err(|_| "Git origin is required".to_owned())
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;

    if !output.status.success() {
        return Err("git command failed".to_owned());
    }

    let value = String::from_utf8(output.stdout)
        .map_err(|_| "git output is not valid UTF-8".to_owned())?
        .trim()
        .to_owned();
    if value.is_empty() {
        Err("git command returned no output".to_owned())
    } else {
        Ok(value)
    }
}

fn github_slug(repository: &str) -> Option<String> {
    const PREFIXES: [&str; 8] = [
        "github:",
        "https://github.com/",
        "http://github.com/",
        "git+https://github.com/",
        "git+ssh://git@github.com/",
        "git://github.com/",
        "git@github.com:",
        "ssh://git@github.com/",
    ];

    let repository = repository.trim();
    let lowercase = repository.to_ascii_lowercase();
    let prefix = PREFIXES
        .into_iter()
        .find(|prefix| lowercase.starts_with(prefix))?;
    let path = &repository[prefix.len()..];
    let path = path.split(['?', '#']).next()?.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let repository = parts.next().filter(|part| !part.is_empty())?;

    if parts.next().is_some() {
        return None;
    }

    Some(format!("{owner}/{repository}"))
}

fn is_gitlab(repository: &str) -> bool {
    let repository = repository.to_ascii_lowercase();
    repository.starts_with("https://gitlab.com/")
        || repository.starts_with("http://gitlab.com/")
        || repository.starts_with("git+ssh://git@gitlab.com/")
        || repository.starts_with("git@gitlab.com:")
        || repository.starts_with("ssh://git@gitlab.com/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const REUSABLE_WORKFLOW: &str = include_str!("../.github/workflows/npm-release.yml");

    const NPM_PACKAGE: &str = r#"{
  "name": "@acme/demo",
  "version": "1.2.3",
  "repository": "https://github.com/acme/demo.git"
}"#;

    #[test]
    fn initializes_node_project_without_lockfile_idempotently() {
        let project = ProjectFixture::new();
        project.write("package.json", NPM_PACKAGE);

        let paths = match init(&project.path).unwrap() {
            InitStatus::Created(paths) => paths,
            status => panic!("unexpected status: {status:?}"),
        };
        assert_eq!(paths.len(), 3);
        assert!(WORKFLOW.contains("@v1.4.0"));
        assert!(!WORKFLOW.contains("@main"));
        assert!(WORKFLOW.contains("github.event.repository.default_branch"));
        assert_eq!(
            fs::read_to_string(project.path.join(RELEASE_MANIFEST_PATH)).unwrap(),
            "{\n  \".\": \"1.2.3\"\n}\n"
        );
        assert!(matches!(init(&project.path), Ok(InitStatus::Unchanged(_))));
    }

    #[test]
    fn reusable_workflow_covers_node_rust_and_turborepo_release_paths() {
        for expected in [
            "turbo run test build",
            "cargo test",
            "release-please-action",
            "Publish released npm packages",
            "release-plz/action",
            "id-token: write",
        ] {
            assert!(
                REUSABLE_WORKFLOW.contains(expected),
                "missing workflow contract: {expected}"
            );
        }
    }

    #[test]
    fn initializes_rust_project() {
        let project = ProjectFixture::new();
        project.rust_package(".", "demo-rs", "0.3.0", true);

        init(&project.path).unwrap();

        let config = fs::read_to_string(project.path.join(RELEASE_CONFIG_PATH)).unwrap();
        assert!(config.contains("\"release-type\": \"rust\""));
        assert!(config.contains("\".\""));
    }

    #[test]
    fn initializes_turborepo_with_node_and_cargo_workspaces() {
        let project = ProjectFixture::new();
        project.write(
            "package.json",
            r#"{
  "private": true,
  "packageManager": "pnpm@10.15.0",
  "workspaces": ["packages/*"]
}"#,
        );
        project.write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
        project.write("turbo.json", "{\"tasks\":{}}\n");
        project.write(
            "packages/js/package.json",
            r#"{
  "name": "@acme/js",
  "version": "1.0.0",
  "repository": {"url": "https://github.com/acme/demo", "directory": "packages/js"}
}"#,
        );
        project.write(
            "packages/ui/package.json",
            r#"{
  "name": "@acme/ui",
  "version": "1.1.0",
  "repository": {"url": "https://github.com/acme/demo", "directory": "packages/ui"}
}"#,
        );
        project.rust_workspace(&["crates/core", "crates/cli"]);
        project.rust_package("crates/core", "acme-core", "0.2.0", false);
        project.rust_package("crates/cli", "acme-cli", "0.2.0", false);

        init(&project.path).unwrap();

        let config = fs::read_to_string(project.path.join(RELEASE_CONFIG_PATH)).unwrap();
        assert!(config.contains("packages/js"));
        assert!(config.contains("packages/ui"));
        assert!(config.contains("crates/core"));
        assert!(config.contains("crates/cli"));
        assert!(config.contains("cargo-workspace"));
        assert!(config.contains("node-workspace"));
    }

    #[test]
    fn explicit_manager_ignores_stale_and_foreign_lockfiles() {
        let project = ProjectFixture::new();
        project.write(
            "package.json",
            &NPM_PACKAGE.replace(
                "\"version\"",
                "\"packageManager\": \"npm@11.5.1\",\n  \"version\"",
            ),
        );
        project.write("package-lock.json", "{}\n");
        project.write("pnpm-lock.yaml", "\n");
        project.write("yarn.lock", "\n");

        assert!(init(&project.path).is_ok());
    }

    #[test]
    fn reports_unsupported_node_manager() {
        let project = ProjectFixture::new();
        project.write(
            "package.json",
            &NPM_PACKAGE.replace(
                "\"version\"",
                "\"packageManager\": \"yarn@4.9.2\",\n  \"version\"",
            ),
        );

        assert!(
            init(&project.path)
                .unwrap_err()
                .contains("yarn is not supported yet")
        );
    }

    #[test]
    fn skips_private_packages_but_requires_a_publishable_target() {
        let project = ProjectFixture::new();
        project.write("package.json", "{\"private\":true}\n");

        assert!(init(&project.path).unwrap_err().contains("no publishable"));
    }

    #[test]
    fn refuses_to_overwrite_existing_workflow() {
        let project = ProjectFixture::new();
        project.write("package.json", NPM_PACKAGE);
        project.write(WORKFLOW_PATH, "custom workflow\n");

        assert!(
            init(&project.path)
                .unwrap_err()
                .contains("refusing to overwrite")
        );
        assert_eq!(
            fs::read_to_string(project.path.join(WORKFLOW_PATH)).unwrap(),
            "custom workflow\n"
        );
        assert!(!project.path.join(RELEASE_CONFIG_PATH).exists());
    }

    #[test]
    fn preserves_existing_release_configuration() {
        let project = ProjectFixture::new();
        project.write("package.json", NPM_PACKAGE);
        project.write(RELEASE_CONFIG_PATH, "{\"custom\":true}\n");

        init(&project.path).unwrap();

        assert_eq!(
            fs::read_to_string(project.path.join(RELEASE_CONFIG_PATH)).unwrap(),
            "{\"custom\":true}\n"
        );
        assert!(project.path.join(RELEASE_MANIFEST_PATH).is_file());
    }

    #[test]
    fn rejects_repository_mismatch() {
        let project = ProjectFixture::new();
        project.write(
            "package.json",
            &NPM_PACKAGE.replace("acme/demo.git", "acme/other.git"),
        );

        assert!(init(&project.path).unwrap_err().contains("does not match"));
    }

    #[test]
    fn rejects_non_public_node_registry() {
        let project = ProjectFixture::new();
        project.write(
            "package.json",
            &NPM_PACKAGE.replace(
                "\"repository\"",
                "\"publishConfig\": {\"registry\": \"https://registry.example.com\"},\n  \"repository\"",
            ),
        );

        assert!(
            init(&project.path)
                .unwrap_err()
                .contains("only https://registry.npmjs.org")
        );
    }

    #[test]
    fn rejects_git_subdirectory() {
        let project = ProjectFixture::new();
        let nested = project.path.join("package");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("package.json"), NPM_PACKAGE).unwrap();

        assert!(init(&nested).unwrap_err().contains("Git repository root"));
    }

    #[test]
    fn normalizes_supported_github_urls() {
        for url in [
            "github:Acme/demo",
            "https://github.com/Acme/demo.git",
            "git+https://github.com/Acme/demo.git",
            "git+ssh://git@github.com/Acme/demo.git",
            "git@github.com:Acme/demo.git",
            "ssh://git@github.com/Acme/demo.git",
        ] {
            assert_eq!(github_slug(url).as_deref(), Some("Acme/demo"));
        }
        assert_eq!(github_slug("https://gitlab.com/acme/demo.git"), None);
    }

    struct ProjectFixture {
        path: PathBuf,
    }

    impl ProjectFixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "jt-release-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();

            let status = Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            let status = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args([
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/acme/demo.git",
                ])
                .status()
                .unwrap();
            assert!(status.success());

            Self { path }
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.path.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn rust_workspace(&self, members: &[&str]) {
            let members = members
                .iter()
                .map(|member| format!("\"{member}\""))
                .collect::<Vec<_>>()
                .join(", ");
            self.write(
                "Cargo.toml",
                &format!("[workspace]\nmembers = [{members}]\nresolver = \"2\"\n"),
            );
        }

        fn rust_package(&self, path: &str, name: &str, version: &str, root: bool) {
            let directory = if path == "." {
                self.path.clone()
            } else {
                self.path.join(path)
            };
            fs::create_dir_all(directory.join("src")).unwrap();
            fs::write(
                directory.join("src/lib.rs"),
                "pub fn ready() -> bool { true }\n",
            )
            .unwrap();
            let manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2024\"\nrepository = \"https://github.com/acme/demo\"\nlicense = \"MIT\"\ndescription = \"test crate\"\n"
            );
            if root {
                self.write("Cargo.toml", &manifest);
            } else {
                fs::write(directory.join("Cargo.toml"), manifest).unwrap();
            }
        }
    }

    impl Drop for ProjectFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }
}
