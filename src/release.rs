use semver::Version;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::node::command::{CommandResult, CommandSpec, Runner, SystemRunner};
use crate::node::error::AppError;

const WORKFLOW_PATH: &str = ".github/workflows/npm-release.yml";
const RELEASE_CONFIG_PATH: &str = "release-please-config.json";
const RELEASE_MANIFEST_PATH: &str = ".release-please-manifest.json";
const GIT_REPOSITORY_ENVIRONMENT: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_IMPLICIT_WORK_TREE",
];

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
    repository: String,
    description: Option<String>,
}

pub fn init(root: &Path) -> Result<InitStatus, String> {
    let runner = SystemRunner;
    let mut prompt = TerminalPrompter;
    init_with(root, io::stdin().is_terminal(), &runner, &mut prompt)
}

fn init_with(
    root: &Path,
    stdin_is_terminal: bool,
    runner: &dyn Runner,
    prompt: &mut dyn BootstrapPrompter,
) -> Result<InitStatus, String> {
    let project = inspect_project(root)?;
    match git_origin(root, runner)? {
        GitOrigin::Github(repository) => {
            validate_project_repository(&project, &repository, "Git origin")?;
            init_project(root, &project)
        }
        GitOrigin::Missing { initialized } => bootstrap_github_repository(
            root,
            &project,
            initialized,
            stdin_is_terminal,
            runner,
            prompt,
        ),
    }
}

fn init_project(root: &Path, project: &Project) -> Result<InitStatus, String> {
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
    let has_node = root.join("package.json").is_file();
    let has_rust = root.join("Cargo.toml").is_file();
    if !has_node && !has_rust {
        return Err(
            "unsupported project; jt repo cicd currently supports Node.js and Rust".to_owned(),
        );
    }

    let mut packages = Vec::new();
    let mut repository = None;
    let mut description = None;
    if has_node {
        packages.extend(inspect_node_packages(
            root,
            &mut repository,
            &mut description,
        )?);
    }
    if has_rust {
        packages.extend(inspect_rust_packages(
            root,
            &mut repository,
            &mut description,
        )?);
    }
    if packages.is_empty() {
        return Err("project has no publishable Node.js packages or Rust crates".to_owned());
    }
    packages.sort_by(|left, right| {
        left.path.cmp(&right.path).then_with(|| {
            release_type_name(left.release_type).cmp(release_type_name(right.release_type))
        })
    });

    let repository = repository
        .ok_or_else(|| "publishable package repository metadata is required".to_owned())?;
    Ok(Project {
        packages,
        repository,
        description,
    })
}

fn inspect_node_packages(
    root: &Path,
    project_repository: &mut Option<String>,
    project_description: &mut Option<String>,
) -> Result<Vec<ReleasePackage>, String> {
    let root_path = root.join("package.json");
    let root_package = read_json_object(&root_path)?;
    record_description(&root_package, project_description);
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
        record_repository(
            package_repository(&package, &package_path)?,
            &package_path,
            project_repository,
        )?;
        record_description(&package, project_description);

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
    project_repository: &mut Option<String>,
    project_description: &mut Option<String>,
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
        record_repository(repository, &manifest_path, project_repository)?;
        if project_description.is_none() {
            *project_description = package
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
        }
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

fn record_repository(
    repository: &str,
    path: &Path,
    project_repository: &mut Option<String>,
) -> Result<(), String> {
    let package_repository = github_slug(repository)
        .ok_or_else(|| format!("{} repository must point to github.com", path.display()))?;
    if let Some(repository) = project_repository {
        if !repository.eq_ignore_ascii_case(&package_repository) {
            return Err(format!(
                "{} repository {package_repository} does not match project repository {repository}",
                path.display()
            ));
        }
    } else {
        *project_repository = Some(package_repository);
    }
    Ok(())
}

fn record_description(package: &Map<String, Value>, project_description: &mut Option<String>) {
    if project_description.is_none() {
        *project_description = package
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
    }
}

fn validate_project_repository(
    project: &Project,
    repository: &str,
    source: &str,
) -> Result<(), String> {
    if project.repository.eq_ignore_ascii_case(repository) {
        Ok(())
    } else {
        Err(format!(
            "package repository {} does not match {source} {repository}",
            project.repository
        ))
    }
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

#[derive(Debug, Eq, PartialEq)]
enum GitOrigin {
    Missing { initialized: bool },
    Github(String),
}

trait BootstrapPrompter {
    fn intro(&mut self) -> Result<(), String>;
    fn confirm(&mut self, question: &str) -> Result<bool, String>;
    fn repository(&mut self, default: &str) -> Result<String, String>;
    fn description(&mut self, default: Option<&str>) -> Result<String, String>;
    fn preview(&mut self, message: &str) -> Result<(), String>;
    fn cancel(&mut self) -> Result<(), String>;
}

struct TerminalPrompter;

impl BootstrapPrompter for TerminalPrompter {
    fn intro(&mut self) -> Result<(), String> {
        cliclack::intro("jt repo cicd")
            .map_err(|error| format!("cannot render repository questionnaire: {error}"))
    }

    fn confirm(&mut self, question: &str) -> Result<bool, String> {
        cliclack::confirm(question)
            .initial_value(false)
            .interact()
            .map_err(|error| format!("cannot read repository confirmation: {error}"))
    }

    fn repository(&mut self, default: &str) -> Result<String, String> {
        cliclack::input("GitHub repository (OWNER/REPO)")
            .default_input(default)
            .validate(|input: &String| validate_github_slug(input).map(|_| ()))
            .interact()
            .map_err(|error| format!("cannot read GitHub repository: {error}"))
    }

    fn description(&mut self, default: Option<&str>) -> Result<String, String> {
        let input = cliclack::input("Description (optional)").required(false);
        let mut input = if let Some(default) = default {
            input.default_input(default)
        } else {
            input
        };
        input
            .interact()
            .map_err(|error| format!("cannot read GitHub repository description: {error}"))
    }

    fn preview(&mut self, message: &str) -> Result<(), String> {
        cliclack::note("Will create", message)
            .map_err(|error| format!("cannot render repository preview: {error}"))
    }

    fn cancel(&mut self) -> Result<(), String> {
        cliclack::outro_cancel("No changes made")
            .map_err(|error| format!("cannot render cancellation: {error}"))
    }
}

fn bootstrap_github_repository(
    root: &Path,
    project: &Project,
    git_initialized: bool,
    stdin_is_terminal: bool,
    runner: &dyn Runner,
    prompt: &mut dyn BootstrapPrompter,
) -> Result<InitStatus, String> {
    if !stdin_is_terminal {
        return Err(
            "Git origin is missing; run `jt repo cicd` in an interactive terminal to create a public GitHub repository"
                .to_owned(),
        );
    }

    ensure_gh(runner, root)?;
    let login = authenticated_github_user(runner, root)?;
    prompt.intro()?;
    if !prompt.confirm("Create a public GitHub repository?")? {
        prompt.cancel()?;
        return Ok(InitStatus::Unchanged(Vec::new()));
    }

    let directory_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "current directory has no valid repository name".to_owned())?;
    let default_repository = format!("{login}/{directory_name}");
    let repository = validate_github_slug(&prompt.repository(&default_repository)?)?;
    validate_project_repository(project, &repository, "planned GitHub repository")?;
    let description = prompt.description(project.description.as_deref())?;
    let description = (!description.trim().is_empty()).then_some(description);
    let mut preview = format!(
        "Public GitHub repository: {repository}\nInitialize local Git repository: {}",
        if git_initialized { "no" } else { "yes" }
    );
    if let Some(description) = &description {
        preview.push_str("\nDescription: ");
        preview.push_str(description);
    }
    prompt.preview(&preview)?;
    if !prompt.confirm("Create repository and continue?")? {
        prompt.cancel()?;
        return Ok(InitStatus::Unchanged(Vec::new()));
    }

    if !git_initialized {
        if let Err(error) = run_required(
            runner,
            git_command(root, ["init"]),
            "initialize Git repository",
        ) {
            return Err(format!(
                "{error}; any local Git state created by git init was retained; GitHub repository was not created"
            ));
        }
    }

    let mut arguments = vec![
        "repo".to_owned(),
        "create".to_owned(),
        repository.clone(),
        "--public".to_owned(),
        "--source=.".to_owned(),
        "--remote=origin".to_owned(),
    ];
    if let Some(description) = &description {
        arguments.push("--description".to_owned());
        arguments.push(description.clone());
    }
    let mut create = isolate_git_environment(CommandSpec::new("gh", arguments).cwd(root));
    create
        .env
        .insert(OsString::from("GH_HOST"), OsString::from("github.com"));
    if let Err(error) = run_required(runner, create, "create GitHub repository") {
        let retained = if git_initialized {
            "existing local Git state was retained"
        } else {
            "new local Git repository was retained"
        };
        return Err(format!(
            "{error}; {retained}; any GitHub repository created by gh was retained"
        ));
    }

    let verified = git_origin(root, runner).map_err(|error| {
        format!(
            "GitHub repository was created, but origin verification failed; local Git and remote state were retained: {error}"
        )
    })?;
    match verified {
        GitOrigin::Github(origin) if origin.eq_ignore_ascii_case(&repository) => {}
        GitOrigin::Github(origin) => {
            return Err(format!(
                "GitHub repository was created, but origin {origin} does not match {repository}; local Git and remote state were retained"
            ));
        }
        GitOrigin::Missing { .. } => {
            return Err(
                "GitHub repository was created, but origin is still missing; local Git and remote state were retained"
                    .to_owned(),
            );
        }
    }

    init_project(root, project).map_err(|error| {
        format!(
            "GitHub repository and origin were created and retained; release initialization failed: {error}"
        )
    })
}

fn ensure_gh(runner: &dyn Runner, root: &Path) -> Result<(), String> {
    let command = CommandSpec::new("gh", ["--version"]).cwd(root);
    let result = runner.run(&command).map_err(|error| match error {
        AppError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => {
            "GitHub CLI (`gh`) is required; install it before running `jt repo cicd`".to_owned()
        }
        error => format!("cannot check GitHub CLI: {error}"),
    })?;
    if !result.success() {
        return Err(
            "GitHub CLI (`gh`) is required; install it before running `jt repo cicd`".to_owned(),
        );
    }

    let auth = runner
        .run(&CommandSpec::new("gh", ["auth", "status", "--hostname", "github.com"]).cwd(root))
        .map_err(|error| format!("cannot check GitHub authentication: {error}"))?;
    if !auth.success() {
        return Err(
            "GitHub CLI is not authenticated for github.com; run `gh auth login` and try again"
                .to_owned(),
        );
    }
    Ok(())
}

fn authenticated_github_user(runner: &dyn Runner, root: &Path) -> Result<String, String> {
    let output = run_required(
        runner,
        CommandSpec::new(
            "gh",
            ["api", "user", "--hostname", "github.com", "--jq", ".login"],
        )
        .cwd(root),
        "read authenticated GitHub user",
    )?;
    let login = output.stdout.trim();
    if login.is_empty() || login.contains(['/', '\n', '\r']) {
        Err("GitHub CLI returned an invalid authenticated user".to_owned())
    } else {
        Ok(login.to_owned())
    }
}

fn run_required(
    runner: &dyn Runner,
    command: CommandSpec,
    action: &str,
) -> Result<CommandResult, String> {
    let output = runner
        .run(&command)
        .map_err(|error| format!("{action}: {error}"))?;
    output
        .require_success(action, &[])
        .map_err(|error| error.to_string())?;
    Ok(output)
}

fn git_origin(root: &Path, runner: &dyn Runner) -> Result<GitOrigin, String> {
    let top_level = runner
        .run(&git_command(root, ["rev-parse", "--show-toplevel"]))
        .map_err(|error| format!("cannot inspect Git repository: {error}"))?;
    if !top_level.success() {
        return match fs::symlink_metadata(root.join(".git")) {
            Ok(_) => {
                let Err(error) = top_level.require_success("inspect existing Git repository", &[])
                else {
                    return Err("cannot inspect existing Git repository".to_owned());
                };
                Err(format!("{error}; existing Git metadata was retained"))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(GitOrigin::Missing { initialized: false })
            }
            Err(error) => Err(format!("cannot inspect local Git metadata: {error}")),
        };
    }
    let top_level = top_level.stdout.trim();
    if top_level.is_empty() {
        return Err("git rev-parse returned no repository root".to_owned());
    }
    let top_level = fs::canonicalize(top_level)
        .map_err(|error| format!("cannot resolve Git repository root: {error}"))?;
    let root = fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve current directory: {error}"))?;

    if root != top_level {
        return Err("current directory must be the Git repository root".to_owned());
    }

    let output = runner
        .run(&git_command(&root, ["remote", "get-url", "origin"]))
        .map_err(|error| format!("cannot inspect Git origin: {error}"))?;
    if !output.success() {
        return Ok(GitOrigin::Missing { initialized: true });
    }
    let origin = output.stdout.trim();
    if origin.is_empty() {
        return Err("Git origin returned no URL".to_owned());
    }
    if is_gitlab(origin) {
        return Err("GitLab release initialization is not supported yet".to_owned());
    }
    let repository =
        github_slug(origin).ok_or_else(|| "Git origin must point to github.com".to_owned())?;
    Ok(GitOrigin::Github(repository))
}

fn git_command(
    root: &Path,
    arguments: impl IntoIterator<Item = impl Into<OsString>>,
) -> CommandSpec {
    isolate_git_environment(CommandSpec::new("git", arguments).cwd(root))
}

fn isolate_git_environment(mut command: CommandSpec) -> CommandSpec {
    command.remove_env.extend(
        GIT_REPOSITORY_ENVIRONMENT
            .iter()
            .copied()
            .map(OsString::from),
    );
    command
}

fn validate_github_slug(value: &str) -> Result<String, String> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repository = parts.next().unwrap_or_default();
    let valid_owner = owner
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && !owner.contains("--");
    let valid_repository = repository
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'));
    if owner.is_empty()
        || repository.is_empty()
        || parts.next().is_some()
        || !valid_owner
        || !valid_repository
        || repository.starts_with('-')
        || matches!(owner, "." | "..")
        || matches!(repository, "." | "..")
    {
        return Err("repository must use OWNER/REPO format".to_owned());
    }
    Ok(format!("{owner}/{repository}"))
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
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        for repository in [
            "acme",
            " acme/demo",
            "acme/demo/extra",
            "-acme/demo",
            "acme-/demo",
            "ac--me/demo",
            "acme/demo?private=true",
        ] {
            assert!(validate_github_slug(repository).is_err());
        }
        assert_eq!(
            validate_github_slug("acme/demo.rs").unwrap(),
            "acme/demo.rs"
        );
    }

    #[test]
    fn missing_origin_cancellations_do_not_mutate_repository() {
        for confirmations in [vec![false], vec![true, false]] {
            let project = ProjectFixture::without_git();
            project.write("package.json", NPM_PACKAGE);
            let runner = BootstrapRunner::new(&project.path);
            let mut prompt = StubPrompt::new(confirmations, vec!["acme/demo"], vec![""]);

            assert_eq!(
                init_with(&project.path, true, &runner, &mut prompt).unwrap(),
                InitStatus::Unchanged(Vec::new())
            );
            assert!(!runner.git_initialized.load(Ordering::Relaxed));
            assert!(runner.origin.lock().unwrap().is_none());
            assert!(!runner.has_call("git", &["init"]));
            assert!(!runner.has_call_prefix("gh", &["repo", "create"]));
            assert!(!project.path.join(WORKFLOW_PATH).exists());
            assert_eq!(prompt.cancelled, 1);
        }
    }

    #[test]
    fn missing_gh_and_authentication_fail_before_prompt_or_mutation() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);

        let mut missing_runner = BootstrapRunner::new(&project.path);
        missing_runner.gh_available = false;
        let mut prompt = StubPrompt::default();
        let error = init_with(&project.path, true, &missing_runner, &mut prompt).unwrap_err();
        assert!(error.contains("GitHub CLI (`gh`) is required"));
        assert_eq!(prompt.intros, 0);
        assert!(!missing_runner.git_initialized.load(Ordering::Relaxed));

        let mut unauthenticated_runner = BootstrapRunner::new(&project.path);
        unauthenticated_runner.authenticated = false;
        let mut prompt = StubPrompt::default();
        let error =
            init_with(&project.path, true, &unauthenticated_runner, &mut prompt).unwrap_err();
        assert!(error.contains("gh auth login"));
        assert_eq!(prompt.intros, 0);
        assert!(
            !unauthenticated_runner
                .git_initialized
                .load(Ordering::Relaxed)
        );
    }

    #[test]
    fn creates_public_repository_with_separate_arguments_then_continues() {
        let project = ProjectFixture::without_git();
        project.write(
            "package.json",
            r#"{
  "private": true,
  "packageManager": "npm@11.5.1",
  "workspaces": ["packages/*"],
  "description": "Package description"
}"#,
        );
        project.write("packages/demo/package.json", NPM_PACKAGE);
        let runner = BootstrapRunner::new(&project.path);
        let mut prompt = StubPrompt::new(
            vec![true, true],
            vec!["acme/demo"],
            vec!["Selected description"],
        );

        let status = init_with(&project.path, true, &runner, &mut prompt).unwrap();

        assert!(matches!(status, InitStatus::Created(_)));
        assert!(runner.git_initialized.load(Ordering::Relaxed));
        assert_eq!(
            runner.origin.lock().unwrap().as_deref(),
            Some("https://github.com/acme/demo.git")
        );
        assert!(runner.has_call("git", &["init"]));
        assert!(runner.has_call(
            "gh",
            &[
                "repo",
                "create",
                "acme/demo",
                "--public",
                "--source=.",
                "--remote=origin",
                "--description",
                "Selected description",
            ],
        ));
        let create = runner
            .calls()
            .into_iter()
            .find(|call| {
                call.program == Path::new("gh")
                    && call
                        .args
                        .starts_with(&[OsString::from("repo"), OsString::from("create")])
            })
            .unwrap();
        assert_eq!(
            create.env.get(&OsString::from("GH_HOST")),
            Some(&OsString::from("github.com"))
        );
        for variable in GIT_REPOSITORY_ENVIRONMENT {
            assert!(create.remove_env.contains(&OsString::from(variable)));
        }
        let git_init = runner
            .calls()
            .into_iter()
            .find(|call| call.program == Path::new("git") && call.args == [OsString::from("init")])
            .unwrap();
        for variable in GIT_REPOSITORY_ENVIRONMENT {
            assert!(git_init.remove_env.contains(&OsString::from(variable)));
        }
        assert!(
            !runner
                .calls()
                .iter()
                .flat_map(|call| call.args.iter())
                .any(|argument| argument == "--push")
        );
        assert_eq!(
            prompt.repository_defaults,
            vec![format!(
                "acme/{}",
                project.path.file_name().unwrap().to_string_lossy()
            )]
        );
        assert_eq!(
            prompt.description_defaults,
            vec![Some("Package description".to_owned())]
        );
        assert!(project.path.join(WORKFLOW_PATH).is_file());
    }

    #[test]
    fn existing_git_repository_without_origin_skips_git_init() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        let runner = BootstrapRunner::new(&project.path);
        runner.git_initialized.store(true, Ordering::Relaxed);
        let mut prompt = StubPrompt::new(vec![true, true], vec!["acme/demo"], vec![""]);

        assert!(init_with(&project.path, true, &runner, &mut prompt).is_ok());
        assert!(!runner.has_call("git", &["init"]));
        assert!(runner.has_call_prefix("gh", &["repo", "create"]));
    }

    #[test]
    fn existing_git_metadata_is_not_reinitialized_when_git_probe_fails() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        fs::create_dir(project.path.join(".git")).unwrap();
        let runner = BootstrapRunner::new(&project.path);
        let mut prompt = StubPrompt::default();

        let error = init_with(&project.path, true, &runner, &mut prompt).unwrap_err();

        assert!(error.contains("existing Git metadata was retained"));
        assert_eq!(prompt.intros, 0);
        assert!(!runner.has_call("git", &["init"]));
        assert!(!runner.has_call_prefix("gh", &["repo", "create"]));
    }

    #[test]
    fn existing_non_github_origin_is_not_overwritten() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        let runner = BootstrapRunner::new(&project.path);
        runner.git_initialized.store(true, Ordering::Relaxed);
        *runner.origin.lock().unwrap() = Some("https://gitlab.com/acme/demo.git".to_owned());
        let mut prompt = StubPrompt::default();

        let error = init_with(&project.path, true, &runner, &mut prompt).unwrap_err();

        assert!(error.contains("GitLab release initialization is not supported"));
        assert_eq!(prompt.intros, 0);
        assert!(!runner.has_call_prefix("gh", &["repo", "create"]));
        assert_eq!(
            runner.origin.lock().unwrap().as_deref(),
            Some("https://gitlab.com/acme/demo.git")
        );
    }

    #[test]
    fn planned_repository_mismatch_stops_before_mutation() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        let runner = BootstrapRunner::new(&project.path);
        let mut prompt = StubPrompt::new(vec![true], vec!["acme/other"], Vec::new());

        let error = init_with(&project.path, true, &runner, &mut prompt).unwrap_err();

        assert!(error.contains("does not match planned GitHub repository"));
        assert!(!runner.git_initialized.load(Ordering::Relaxed));
        assert!(!runner.has_call_prefix("gh", &["repo", "create"]));
    }

    #[test]
    fn failed_creation_retains_new_local_git_repository() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        let mut runner = BootstrapRunner::new(&project.path);
        runner.create_status = 1;
        let mut prompt = StubPrompt::new(vec![true, true], vec!["acme/demo"], vec![""]);

        let error = init_with(&project.path, true, &runner, &mut prompt).unwrap_err();

        assert!(error.contains("new local Git repository was retained"));
        assert!(error.contains("GitHub repository created by gh was retained"));
        assert!(runner.git_initialized.load(Ordering::Relaxed));
        assert!(runner.origin.lock().unwrap().is_none());
        assert!(!project.path.join(WORKFLOW_PATH).exists());
    }

    #[test]
    fn failed_git_init_reports_retained_state_without_creating_github_repository() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        let mut runner = BootstrapRunner::new(&project.path);
        runner.git_init_status = 1;
        let mut prompt = StubPrompt::new(vec![true, true], vec!["acme/demo"], vec![""]);

        let error = init_with(&project.path, true, &runner, &mut prompt).unwrap_err();

        assert!(error.contains("local Git state created by git init was retained"));
        assert!(error.contains("GitHub repository was not created"));
        assert!(!runner.has_call_prefix("gh", &["repo", "create"]));
    }

    #[test]
    fn origin_verification_failure_retains_created_state() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        let mut runner = BootstrapRunner::new(&project.path);
        runner.created_origin = "https://github.com/acme/other.git".to_owned();
        let mut prompt = StubPrompt::new(vec![true, true], vec!["acme/demo"], vec![""]);

        let error = init_with(&project.path, true, &runner, &mut prompt).unwrap_err();

        assert!(error.contains("origin acme/other does not match acme/demo"));
        assert!(error.contains("state were retained"));
        assert!(runner.git_initialized.load(Ordering::Relaxed));
        assert_eq!(
            runner.origin.lock().unwrap().as_deref(),
            Some("https://github.com/acme/other.git")
        );
        assert!(!project.path.join(WORKFLOW_PATH).exists());
    }

    #[test]
    fn release_initialization_failure_retains_created_repository_and_origin() {
        let project = ProjectFixture::without_git();
        project.write("package.json", NPM_PACKAGE);
        project.write(WORKFLOW_PATH, "custom workflow\n");
        let runner = BootstrapRunner::new(&project.path);
        let mut prompt = StubPrompt::new(vec![true, true], vec!["acme/demo"], vec![""]);

        let error = init_with(&project.path, true, &runner, &mut prompt).unwrap_err();

        assert!(error.contains("repository and origin were created and retained"));
        assert!(runner.git_initialized.load(Ordering::Relaxed));
        assert_eq!(
            runner.origin.lock().unwrap().as_deref(),
            Some("https://github.com/acme/demo.git")
        );
        assert_eq!(
            fs::read_to_string(project.path.join(WORKFLOW_PATH)).unwrap(),
            "custom workflow\n"
        );
        assert!(!project.path.join(RELEASE_CONFIG_PATH).exists());
    }

    #[derive(Default)]
    struct StubPrompt {
        confirmations: VecDeque<bool>,
        repositories: VecDeque<String>,
        descriptions: VecDeque<String>,
        repository_defaults: Vec<String>,
        description_defaults: Vec<Option<String>>,
        intros: usize,
        cancelled: usize,
    }

    impl StubPrompt {
        fn new(confirmations: Vec<bool>, repositories: Vec<&str>, descriptions: Vec<&str>) -> Self {
            Self {
                confirmations: confirmations.into(),
                repositories: repositories.into_iter().map(str::to_owned).collect(),
                descriptions: descriptions.into_iter().map(str::to_owned).collect(),
                ..Self::default()
            }
        }
    }

    impl BootstrapPrompter for StubPrompt {
        fn intro(&mut self) -> Result<(), String> {
            self.intros += 1;
            Ok(())
        }

        fn confirm(&mut self, _: &str) -> Result<bool, String> {
            self.confirmations
                .pop_front()
                .ok_or_else(|| "missing confirmation".to_owned())
        }

        fn repository(&mut self, default: &str) -> Result<String, String> {
            self.repository_defaults.push(default.to_owned());
            self.repositories
                .pop_front()
                .ok_or_else(|| "missing repository".to_owned())
        }

        fn description(&mut self, default: Option<&str>) -> Result<String, String> {
            self.description_defaults.push(default.map(str::to_owned));
            self.descriptions
                .pop_front()
                .ok_or_else(|| "missing description".to_owned())
        }

        fn preview(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }

        fn cancel(&mut self) -> Result<(), String> {
            self.cancelled += 1;
            Ok(())
        }
    }

    struct BootstrapRunner {
        root: PathBuf,
        calls: Mutex<Vec<CommandSpec>>,
        git_initialized: AtomicBool,
        origin: Mutex<Option<String>>,
        gh_available: bool,
        authenticated: bool,
        git_init_status: i32,
        create_status: i32,
        created_origin: String,
    }

    impl BootstrapRunner {
        fn new(root: &Path) -> Self {
            Self {
                root: fs::canonicalize(root).unwrap(),
                calls: Mutex::new(Vec::new()),
                git_initialized: AtomicBool::new(false),
                origin: Mutex::new(None),
                gh_available: true,
                authenticated: true,
                git_init_status: 0,
                create_status: 0,
                created_origin: "https://github.com/acme/demo.git".to_owned(),
            }
        }

        fn calls(&self) -> Vec<CommandSpec> {
            self.calls.lock().unwrap().clone()
        }

        fn has_call(&self, program: &str, expected: &[&str]) -> bool {
            self.calls().iter().any(|call| {
                call.program == Path::new(program)
                    && call
                        .args
                        .iter()
                        .map(|value| value.to_string_lossy())
                        .eq(expected.iter().copied())
            })
        }

        fn has_call_prefix(&self, program: &str, expected: &[&str]) -> bool {
            self.calls().iter().any(|call| {
                call.program == Path::new(program)
                    && call.args.len() >= expected.len()
                    && call
                        .args
                        .iter()
                        .map(|value| value.to_string_lossy())
                        .zip(expected.iter().copied())
                        .all(|(actual, expected)| actual == expected)
            })
        }
    }

    impl Runner for BootstrapRunner {
        fn run(&self, command: &CommandSpec) -> crate::node::error::Result<CommandResult> {
            self.calls.lock().unwrap().push(command.clone());
            let arguments = command
                .args
                .iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>();
            let arguments = arguments
                .iter()
                .map(|value| value.as_ref())
                .collect::<Vec<_>>();
            let success = |stdout: String| CommandResult {
                status: 0,
                stdout,
                stderr: String::new(),
            };
            let failure = |detail: &str| CommandResult {
                status: 1,
                stdout: String::new(),
                stderr: detail.to_owned(),
            };

            match (command.program.to_str(), arguments.as_slice()) {
                (Some("git"), ["rev-parse", "--show-toplevel"])
                    if self.git_initialized.load(Ordering::Relaxed) =>
                {
                    Ok(success(format!("{}\n", self.root.display())))
                }
                (Some("git"), ["rev-parse", "--show-toplevel"]) => {
                    Ok(failure("not a git repository"))
                }
                (Some("git"), ["remote", "get-url", "origin"]) => {
                    let origin = self.origin.lock().unwrap();
                    Ok(match origin.as_deref() {
                        Some(origin) => success(format!("{origin}\n")),
                        None => failure("origin is missing"),
                    })
                }
                (Some("git"), ["init"]) if self.git_init_status != 0 => {
                    Ok(failure("git initialization failed"))
                }
                (Some("git"), ["init"]) => {
                    self.git_initialized.store(true, Ordering::Relaxed);
                    Ok(success(String::new()))
                }
                (Some("gh"), ["--version"]) if !self.gh_available => Err(AppError::io(
                    "start command",
                    Some(PathBuf::from("gh")),
                    io::Error::new(io::ErrorKind::NotFound, "missing gh"),
                )),
                (Some("gh"), ["--version"]) => Ok(success("gh version test\n".to_owned())),
                (Some("gh"), ["auth", "status", "--hostname", "github.com"])
                    if self.authenticated =>
                {
                    Ok(success(String::new()))
                }
                (Some("gh"), ["auth", "status", "--hostname", "github.com"]) => {
                    Ok(failure("not logged in"))
                }
                (Some("gh"), ["api", "user", "--hostname", "github.com", "--jq", ".login"]) => {
                    Ok(success("acme\n".to_owned()))
                }
                (Some("gh"), ["repo", "create", ..]) if self.create_status != 0 => {
                    Ok(failure("repository creation failed"))
                }
                (Some("gh"), ["repo", "create", ..]) => {
                    *self.origin.lock().unwrap() = Some(self.created_origin.clone());
                    Ok(success(String::new()))
                }
                _ => panic!("unexpected command: {command:?}"),
            }
        }
    }

    struct ProjectFixture {
        path: PathBuf,
    }

    impl ProjectFixture {
        fn new() -> Self {
            let project = Self::without_git();

            let status = Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(&project.path)
                .status()
                .unwrap();
            assert!(status.success());
            let status = Command::new("git")
                .arg("-C")
                .arg(&project.path)
                .args([
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/acme/demo.git",
                ])
                .status()
                .unwrap();
            assert!(status.success());

            project
        }

        fn without_git() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "jt-release-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();

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
