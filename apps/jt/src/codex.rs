use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use serde_json::Value;

const ENVIRONMENT: &str = ".codex/environments/environment.toml";
const CODEGRAPH_SETUP: &str = r#"
if command -v codegraph >/dev/null 2>&1; then
  worktree_root="$(git rev-parse --show-toplevel)"
  main_worktree_root="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"
  if [ -L .codegraph ] || [ -L .codegraph/codegraph.db ]; then
    printf '%s\n' 'Refusing shared CodeGraph symlink; each worktree needs its own database.' >&2
    exit 1
  fi
  if [ ! -f .codegraph/codegraph.db ] \
    && [ "$worktree_root" != "$main_worktree_root" ] \
    && [ -f "$main_worktree_root/.codegraph/codegraph.db" ] \
    && command -v sqlite3 >/dev/null 2>&1; then
    mkdir -p .codegraph
    (cd .codegraph && sqlite3 "$main_worktree_root/.codegraph/codegraph.db" '.backup codegraph.db')
  fi
  if [ -f .codegraph/codegraph.db ]; then
    codegraph sync "$worktree_root"
  else
    codegraph init "$worktree_root"
  fi
else
  printf '%s\n' 'CodeGraph unavailable; existing project index was not prepared.' >&2
fi
"#;

pub fn run() -> u8 {
    let result = std::env::current_dir()
        .map_err(|error| error.to_string())
        .and_then(|root| init(&root));
    match result {
        Ok(true) => println!("created {ENVIRONMENT}; setup runs when Codex creates a worktree"),
        Ok(false) => println!("preserved existing {ENVIRONMENT}; no changes made"),
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    }
    0
}

fn init(root: &Path) -> Result<bool, String> {
    for relative in [".codex", ".codex/environments", ENVIRONMENT] {
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!("refusing symlink: {}", path.display()));
                }
                if relative == ENVIRONMENT {
                    if !metadata.is_file() {
                        return Err(format!("not a regular file: {}", path.display()));
                    }
                    return Ok(false);
                }
                if !metadata.is_dir() {
                    return Err(format!("not a directory: {}", path.display()));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
        }
    }
    let content = render(root)?;
    let directory = root.join(".codex/environments");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    file.write_all(content.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    file.persist_noclobber(root.join(ENVIRONMENT))
        .map_err(|error| format!("cannot create {ENVIRONMENT}: {error}"))?;
    Ok(true)
}

fn render(root: &Path) -> Result<String, String> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let mut setup = String::from("set -eu\n");
    let mut actions = String::new();
    match fs::read(root.join("package.json")) {
        Ok(bytes) => {
            let package: Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid package.json: {error}"))?;
            if !package.is_object() {
                return Err("package.json must be an object".to_owned());
            }
            let manager = package_manager(root, &package)?;
            let install = match manager {
                "pnpm" if root.join("pnpm-lock.yaml").is_file() => {
                    "pnpm install --frozen-lockfile --prefer-offline"
                }
                "pnpm" => "pnpm install --prefer-offline",
                _ if root.join("package-lock.json").is_file()
                    || root.join("npm-shrinkwrap.json").is_file() =>
                {
                    "npm ci"
                }
                _ => "npm install",
            };
            setup.push_str(install);
            setup.push('\n');
            let scripts = match package.get("scripts") {
                None => None,
                Some(Value::Object(scripts)) if scripts.values().all(Value::is_string) => {
                    Some(scripts)
                }
                Some(_) => {
                    return Err("package.json scripts must be an object of strings".to_owned());
                }
            };
            for (name, icon, candidates) in [
                ("Dev", "run", &["dev", "start"][..]),
                (
                    "Unit tests",
                    "test",
                    &["test:unit:run", "test:unit", "test"][..],
                ),
                (
                    "Type check",
                    "test",
                    &["type-check", "typecheck", "type:check"][..],
                ),
            ] {
                if let Some(script) = candidates.iter().find(|script| {
                    scripts
                        .and_then(|scripts| scripts.get(**script))
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty())
                }) {
                    actions.push_str(&format!(
                        "\n[[actions]]\nname = {}\nicon = {}\ncommand = {}\n",
                        quote(name),
                        quote(icon),
                        quote(&format!("{manager} run {script}"))
                    ));
                }
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot read package.json: {error}")),
    }
    if root.join(".codegraph/codegraph.db").is_file() {
        setup.push_str(CODEGRAPH_SETUP);
    }
    Ok(format!(
        "# Generated by jt codex init. Safe to customize; reruns preserve this file.\nversion = 1\nname = {}\n\n[setup]\nscript = '''\n{}'''\n{actions}",
        quote(name),
        setup
    ))
}

fn package_manager(root: &Path, package: &Value) -> Result<&'static str, String> {
    if let Some(declared) = package.get("packageManager") {
        return match declared.as_str().and_then(|value| value.split_once('@')) {
            Some(("pnpm", version)) if !version.is_empty() => Ok("pnpm"),
            Some(("npm", version)) if !version.is_empty() => Ok("npm"),
            _ => Err("supported packageManager values: npm@<version>, pnpm@<version>".to_owned()),
        };
    }
    let managers: Vec<_> = [
        ("pnpm", root.join("pnpm-lock.yaml").is_file()),
        (
            "npm",
            root.join("package-lock.json").is_file() || root.join("npm-shrinkwrap.json").is_file(),
        ),
        ("yarn", root.join("yarn.lock").is_file()),
        (
            "bun",
            root.join("bun.lock").is_file() || root.join("bun.lockb").is_file(),
        ),
    ]
    .into_iter()
    .filter_map(|(name, exists)| exists.then_some(name))
    .collect();
    match managers.as_slice() {
        [] | ["npm"] => Ok("npm"),
        ["pnpm"] => Ok("pnpm"),
        _ => Err("unsupported or conflicting lockfiles; declare npm@<version> or pnpm@<version> in packageManager".to_owned()),
    }
}

// Escape DEL as well: TOML basic strings forbid this raw control character.
fn quote(value: &str) -> String {
    serde_json::to_string(value)
        .unwrap()
        .replace('\u{7f}', "\\u007f")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generates_only_available_actions_and_preserves_existing_configuration() {
        let project = tempdir().unwrap();
        fs::write(project.path().join("package.json"), r#"{"scripts":{"dev":"vite", "test:unit:run":"vitest run", "type-check":"vue-tsc", "unused":"ignored"}}"#).unwrap();
        fs::write(project.path().join("pnpm-lock.yaml"), "").unwrap();
        assert!(init(project.path()).unwrap());
        let path = project.path().join(ENVIRONMENT);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("pnpm install --frozen-lockfile --prefer-offline"));
        assert!(content.contains("pnpm run dev"));
        assert!(content.contains("pnpm run test:unit:run"));
        assert!(content.contains("pnpm run type-check"));
        assert!(!content.contains("unused"));
        assert!(!content.contains("codegraph"));
        assert!(!project.path().join("node_modules").exists());
        assert!(!init(project.path()).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        fs::write(&path, "custom configuration, even unknown syntax\n").unwrap();
        fs::write(project.path().join("package.json"), "invalid").unwrap();
        assert!(!init(project.path()).unwrap());
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "custom configuration, even unknown syntax\n"
        );
    }

    #[test]
    fn detects_supported_managers_and_omits_missing_scripts() {
        let project = tempdir().unwrap();
        for (package, lock, expected) in [
            (r#"{}"#, "package-lock.json", "npm ci"),
            (r#"{}"#, "npm-shrinkwrap.json", "npm ci"),
            (r#"{}"#, "", "npm install"),
            (
                r#"{"packageManager":"pnpm@10.0.0"}"#,
                "",
                "pnpm install --prefer-offline",
            ),
        ] {
            fs::write(project.path().join("package.json"), package).unwrap();
            if !lock.is_empty() {
                fs::write(project.path().join(lock), "").unwrap();
            }
            let content = render(project.path()).unwrap();
            assert!(content.contains(expected), "{content}");
            assert!(!content.contains("[[actions]]"));
            if !lock.is_empty() {
                fs::remove_file(project.path().join(lock)).unwrap();
            }
        }
        fs::write(project.path().join("package.json"), "{}").unwrap();
        fs::write(project.path().join("yarn.lock"), "").unwrap();
        assert!(init(project.path()).is_err());
        assert!(!project.path().join(".codex").exists());
        fs::remove_file(project.path().join("yarn.lock")).unwrap();
        fs::write(project.path().join("package-lock.json"), "").unwrap();
        fs::write(project.path().join("pnpm-lock.yaml"), "").unwrap();
        assert!(render(project.path()).is_err());
    }

    #[test]
    fn codegraph_requires_an_existing_database() {
        let project = tempdir().unwrap();
        fs::create_dir(project.path().join(".codegraph")).unwrap();
        assert!(!render(project.path()).unwrap().contains("codegraph"));
        fs::write(project.path().join(".codegraph/codegraph.db"), "").unwrap();
        let content = render(project.path()).unwrap();
        let script = content
            .split_once("script = '''\n")
            .unwrap()
            .1
            .split_once("'''")
            .unwrap()
            .0;
        assert!(script.contains(".backup codegraph.db"));
        assert!(script.contains("codegraph sync"));
        assert!(script.contains("codegraph init"));
        assert!(
            std::process::Command::new("sh")
                .args(["-n", "-c", script])
                .status()
                .unwrap()
                .success()
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_configuration_paths() {
        use std::os::unix::fs::symlink;
        for relative in [".codex", ".codex/environments", ENVIRONMENT] {
            let project = tempdir().unwrap();
            let outside = tempdir().unwrap();
            let path = project.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink(outside.path().join("missing"), &path).unwrap();
            assert!(init(project.path()).unwrap_err().contains("symlink"));
            assert!(!outside.path().join("missing").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn setup_copies_an_independent_database_and_syncs_the_worktree() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::process::Command;

        let directory = tempdir().unwrap();
        let primary = directory.path().join("primary ' project");
        let worktree = directory.path().join("worktree ' project");
        fs::create_dir(&primary).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
            vec!["worktree", "add", "--detach", worktree.to_str().unwrap()],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&primary)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        fs::create_dir(primary.join(".codegraph")).unwrap();
        let source = primary.join(".codegraph/codegraph.db");
        let database = rusqlite::Connection::open(&source).unwrap();
        database.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE evidence (value TEXT); INSERT INTO evidence VALUES ('original');").unwrap();
        let bin = directory.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let codegraph = bin.join("codegraph");
        fs::write(
            &codegraph,
            "#!/bin/sh\nprintf '%s\\n' \"$1\" > codegraph-action\n",
        )
        .unwrap();
        fs::set_permissions(&codegraph, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let content = render(&primary).unwrap();
        let script = content
            .split_once("script = '''\n")
            .unwrap()
            .1
            .split_once("'''")
            .unwrap()
            .0;
        assert!(
            Command::new("sh")
                .args(["-c", script])
                .env("PATH", path)
                .current_dir(&worktree)
                .status()
                .unwrap()
                .success()
        );
        let destination = worktree.join(".codegraph/codegraph.db");
        assert_ne!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&destination).unwrap().ino()
        );
        let copy = rusqlite::Connection::open(destination).unwrap();
        assert_eq!(
            copy.query_row("SELECT value FROM evidence", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "original"
        );
        copy.execute("UPDATE evidence SET value = 'worktree'", [])
            .unwrap();
        assert_eq!(
            database
                .query_row("SELECT value FROM evidence", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(worktree.join("codegraph-action")).unwrap(),
            "sync\n"
        );
    }
}
