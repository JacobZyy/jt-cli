use semver::Version;
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const WORKFLOW_PATH: &str = ".github/workflows/npm-release.yml";

const WORKFLOW: &str = r#"name: npm release

on:
  push:
    branches:
      - main

permissions:
  contents: write
  issues: write
  pull-requests: write
  id-token: write

jobs:
  release:
    uses: JacobZyy/jt-cli/.github/workflows/npm-release.yml@v1.0.0
"#;

#[derive(Debug, Eq, PartialEq)]
pub enum InitStatus {
    Created(PathBuf),
    Unchanged(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageManager {
    Npm,
    Pnpm,
}

pub fn init(root: &Path) -> Result<InitStatus, String> {
    validate_project(root)?;

    let path = root.join(WORKFLOW_PATH);
    if path.exists() {
        let current = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        return if current == WORKFLOW {
            Ok(InitStatus::Unchanged(path))
        } else {
            Err(format!(
                "{} already exists; refusing to overwrite it",
                path.display()
            ))
        };
    }

    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid workflow path: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    if let Err(error) = file
        .write_all(WORKFLOW.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(format!("cannot write {}: {error}", path.display()));
    }

    Ok(InitStatus::Created(path))
}

fn validate_project(root: &Path) -> Result<(), String> {
    let package_path = root.join("package.json");
    let source = fs::read_to_string(&package_path)
        .map_err(|error| format!("cannot read {}: {error}", package_path.display()))?;
    let package = serde_json::from_str::<Value>(&source)
        .map_err(|error| format!("invalid {}: {error}", package_path.display()))?;
    let package = package
        .as_object()
        .ok_or_else(|| "package.json root must be an object".to_owned())?;

    required_string(package, "name")?;
    let version = required_string(package, "version")?;
    Version::parse(version).map_err(|error| format!("package.json version is invalid: {error}"))?;

    if let Some(private) = package.get("private") {
        match private.as_bool() {
            Some(true) => return Err("package.json private must not be true".to_owned()),
            Some(false) => {}
            None => return Err("package.json private must be a boolean".to_owned()),
        }
    }

    if package.contains_key("workspaces") {
        return Err("npm workspaces are not supported yet".to_owned());
    }

    let scripts = package
        .get("scripts")
        .and_then(Value::as_object)
        .ok_or_else(|| "package.json scripts must be an object".to_owned())?;
    required_string(scripts, "test")
        .map_err(|_| "package.json scripts.test must be a non-empty string".to_owned())?;
    required_string(scripts, "build")
        .map_err(|_| "package.json scripts.build must be a non-empty string".to_owned())?;

    validate_registry(package)?;
    let manager = detect_package_manager(root)?;
    validate_package_manager(package, manager)?;

    let package_repository = package_repository(package)?;
    let origin = git_origin(root)?;

    if is_gitlab(&origin) {
        return Err("GitLab release initialization is not supported yet".to_owned());
    }

    let origin_repository =
        github_slug(&origin).ok_or_else(|| "Git origin must point to github.com".to_owned())?;
    let package_repository = github_slug(package_repository)
        .ok_or_else(|| "package.json repository must point to github.com".to_owned())?;

    if !origin_repository.eq_ignore_ascii_case(&package_repository) {
        return Err(format!(
            "package.json repository {package_repository} does not match Git origin {origin_repository}"
        ));
    }

    Ok(())
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("package.json {key} must be a non-empty string"))
}

fn validate_registry(package: &Map<String, Value>) -> Result<(), String> {
    let Some(publish_config) = package.get("publishConfig") else {
        return Ok(());
    };
    let publish_config = publish_config
        .as_object()
        .ok_or_else(|| "package.json publishConfig must be an object".to_owned())?;
    let Some(registry) = publish_config.get("registry") else {
        return Ok(());
    };
    let registry = registry
        .as_str()
        .ok_or_else(|| "package.json publishConfig.registry must be a string".to_owned())?;

    if registry.trim_end_matches('/') != "https://registry.npmjs.org" {
        return Err("only https://registry.npmjs.org is supported".to_owned());
    }

    Ok(())
}

fn detect_package_manager(root: &Path) -> Result<PackageManager, String> {
    let npm = root.join("package-lock.json").is_file();
    let pnpm = root.join("pnpm-lock.yaml").is_file();
    let unsupported = ["yarn.lock", "bun.lock", "bun.lockb", "deno.lock"]
        .into_iter()
        .filter(|name| root.join(name).exists())
        .collect::<Vec<_>>();

    if !unsupported.is_empty() {
        return Err(format!("unsupported lockfile: {}", unsupported.join(", ")));
    }

    match (npm, pnpm) {
        (true, false) => Ok(PackageManager::Npm),
        (false, true) => Ok(PackageManager::Pnpm),
        (true, true) => {
            Err("expected exactly one of package-lock.json or pnpm-lock.yaml".to_owned())
        }
        (false, false) => Err("expected package-lock.json or pnpm-lock.yaml".to_owned()),
    }
}

fn validate_package_manager(
    package: &Map<String, Value>,
    manager: PackageManager,
) -> Result<(), String> {
    let declared = package.get("packageManager").map(|value| {
        value
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "package.json packageManager must be a non-empty string".to_owned())
    });

    match (manager, declared) {
        (PackageManager::Npm, None) => Ok(()),
        (PackageManager::Npm, Some(Ok(value))) if value.starts_with("npm@") => {
            let version = value.trim_start_matches("npm@");
            Version::parse(version)
                .map(|_| ())
                .map_err(|error| format!("packageManager npm version is invalid: {error}"))
        }
        (PackageManager::Npm, Some(Ok(_))) => {
            Err("packageManager conflicts with package-lock.json".to_owned())
        }
        (PackageManager::Pnpm, Some(Ok(value))) if value.starts_with("pnpm@") => {
            let version = value.trim_start_matches("pnpm@");
            Version::parse(version)
                .map(|_| ())
                .map_err(|error| format!("packageManager pnpm version is invalid: {error}"))
        }
        (PackageManager::Pnpm, Some(Ok(_))) => {
            Err("packageManager conflicts with pnpm-lock.yaml".to_owned())
        }
        (PackageManager::Pnpm, None) => {
            Err("pnpm projects require packageManager, for example pnpm@10.15.0".to_owned())
        }
        (_, Some(Err(error))) => Err(error),
    }
}

fn package_repository(package: &Map<String, Value>) -> Result<&str, String> {
    match package.get("repository") {
        Some(Value::String(repository)) if !repository.trim().is_empty() => Ok(repository),
        Some(Value::Object(repository)) => {
            if repository.contains_key("directory") {
                return Err("repository.directory is not supported yet".to_owned());
            }
            repository
                .get("url")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "package.json repository.url must be a non-empty string".to_owned())
        }
        _ => Err("package.json repository is required".to_owned()),
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

    const NPM_PACKAGE: &str = r#"{
  "name": "@acme/demo",
  "version": "1.2.3",
  "repository": "https://github.com/acme/demo.git",
  "scripts": {
    "test": "node --test",
    "build": "node build.js"
  }
}"#;

    const PNPM_PACKAGE: &str = r#"{
  "name": "@acme/demo",
  "version": "1.2.3",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/acme/demo.git"
  },
  "packageManager": "pnpm@10.15.0",
  "scripts": {
    "test": "node --test",
    "build": "node build.js"
  }
}"#;

    #[test]
    fn initializes_npm_project_idempotently() {
        let project = Project::new(NPM_PACKAGE, "package-lock.json");

        let path = project.path.join(WORKFLOW_PATH);
        assert_eq!(init(&project.path), Ok(InitStatus::Created(path.clone())));
        assert_eq!(fs::read_to_string(&path).unwrap(), WORKFLOW);
        assert!(WORKFLOW.contains("@v1.0.0"));
        assert!(!WORKFLOW.contains("@main"));
        assert_eq!(init(&project.path), Ok(InitStatus::Unchanged(path)));
    }

    #[test]
    fn initializes_pnpm_project() {
        let project = Project::new(PNPM_PACKAGE, "pnpm-lock.yaml");

        assert!(matches!(init(&project.path), Ok(InitStatus::Created(_))));
    }

    #[test]
    fn rejects_missing_conflicting_and_unknown_lockfiles() {
        let missing = Project::new(NPM_PACKAGE, "");
        assert!(
            init(&missing.path)
                .unwrap_err()
                .contains("expected package-lock")
        );

        let conflicting = Project::new(NPM_PACKAGE, "package-lock.json");
        fs::write(conflicting.path.join("pnpm-lock.yaml"), "").unwrap();
        assert!(init(&conflicting.path).unwrap_err().contains("exactly one"));

        let unknown = Project::new(NPM_PACKAGE, "yarn.lock");
        assert!(
            init(&unknown.path)
                .unwrap_err()
                .contains("unsupported lockfile")
        );
    }

    #[test]
    fn refuses_to_overwrite_existing_workflow() {
        let project = Project::new(NPM_PACKAGE, "package-lock.json");
        let path = project.path.join(WORKFLOW_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "custom workflow\n").unwrap();

        assert!(
            init(&project.path)
                .unwrap_err()
                .contains("refusing to overwrite")
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "custom workflow\n");
    }

    #[test]
    fn rejects_repository_mismatch() {
        let package = NPM_PACKAGE.replace("acme/demo.git", "acme/other.git");
        let project = Project::new(&package, "package-lock.json");

        assert!(init(&project.path).unwrap_err().contains("does not match"));
    }

    #[test]
    fn rejects_invalid_package_manager_version() {
        let package = NPM_PACKAGE.replace(
            "\"scripts\"",
            "\"packageManager\": \"npm@garbage\",\n  \"scripts\"",
        );
        let project = Project::new(&package, "package-lock.json");

        assert!(
            init(&project.path)
                .unwrap_err()
                .contains("npm version is invalid")
        );
    }

    #[test]
    fn rejects_git_subdirectory() {
        let project = Project::new(NPM_PACKAGE, "package-lock.json");
        let nested = project.path.join("package");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("package.json"), NPM_PACKAGE).unwrap();
        fs::write(nested.join("package-lock.json"), "").unwrap();

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

    struct Project {
        path: PathBuf,
    }

    impl Project {
        fn new(package: &str, lockfile: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "jt-release-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("package.json"), package).unwrap();
            if !lockfile.is_empty() {
                fs::write(path.join(lockfile), "").unwrap();
            }

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
    }

    impl Drop for Project {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }
}
