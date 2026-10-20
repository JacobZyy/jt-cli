use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::Value;

mod migration;
pub mod recycle;

use migration::{ENVIRONMENT, Mode, Outcome};

const CODEGRAPH_SETUP: &str = include_str!("codex/codegraph.sh");
const ROOTS: &str = r#"worktree_root="$(pwd -P)"
main_worktree_root="$worktree_root"
git_common_dir="$worktree_root/.codex/cache"
if git rev-parse --show-toplevel >/dev/null 2>&1; then
  worktree_root="$(git rev-parse --show-toplevel)"
  main_worktree_root="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"
  git_common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
fi
"#;

pub fn run(check: bool, update: bool, lightweight: bool) -> u8 {
    let mode = if check {
        Mode::Check
    } else if update {
        Mode::Update
    } else {
        Mode::Preserve
    };
    let result = std::env::current_dir()
        .map_err(|error| error.to_string())
        .and_then(|root| {
            if mode == Mode::Preserve && migration::preserve_existing(&root)? {
                return Ok(Outcome::Preserved);
            }
            migration::apply(&root, &render_with_options(&root, lightweight)?, mode)
        });
    match result {
        Ok(Outcome::Created) => {
            println!("created {ENVIRONMENT}; setup runs when Codex creates a worktree")
        }
        Ok(Outcome::Preserved) => {
            println!("preserved existing {ENVIRONMENT}; use --check to review an update")
        }
        Ok(Outcome::Current) => println!("current {ENVIRONMENT}; no changes made"),
        Ok(Outcome::Preview { candidate, managed }) => println!(
            "candidate: {}; managed template: {managed}; review before --update",
            candidate.display()
        ),
        Ok(Outcome::Updated { backup }) => {
            println!("updated {ENVIRONMENT}; restore from {}", backup.display())
        }
        Ok(Outcome::Protected { candidate }) => println!(
            "preserved custom {ENVIRONMENT}; review candidate {} and apply changes manually",
            candidate.display()
        ),
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    }
    0
}

#[cfg(test)]
fn init(root: &Path) -> Result<bool, String> {
    if migration::preserve_existing(root)? {
        return Ok(false);
    }
    Ok(matches!(
        migration::apply(root, &render(root)?, Mode::Preserve)?,
        Outcome::Created
    ))
}

#[cfg(test)]
fn render(root: &Path) -> Result<String, String> {
    render_with_options(root, false)
}

fn render_with_options(root: &Path, lightweight: bool) -> Result<String, String> {
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
            setup.push_str(ROOTS);
            setup.push_str(&format!("# Fallback: {install}\nnode --input-type=module - \"$worktree_root\" \"$main_worktree_root\" {install} <<'JT_CODEX_DEPENDENCIES'\n"));
            setup.push_str(include_str!("codex/dependencies.mjs"));
            setup.push_str("\nJT_CODEX_DEPENDENCIES\n");
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
    if root.join("Cargo.toml").is_file() {
        for (name, icon, command) in [("Build", "run", "build"), ("Rust tests", "test", "test")] {
            let locked = if root.join("Cargo.lock").is_file() {
                " --locked"
            } else {
                ""
            };
            let mut action = format!(
                "set -eu\n{ROOTS}cache=\"$git_common_dir/jt-codex\"\nif [ -L \"$cache\" ] || [ -L \"$cache/cargo-target\" ]; then echo 'Refusing symlinked Cargo cache' >&2; exit 1; fi\n"
            );
            action
                .push_str("export CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-$cache/cargo-target}\"\n");
            if lightweight {
                action.push_str("export CARGO_INCREMENTAL=\"${CARGO_INCREMENTAL:-0}\"\nexport CARGO_PROFILE_DEV_DEBUG=\"${CARGO_PROFILE_DEV_DEBUG:-0}\"\nexport CARGO_PROFILE_TEST_DEBUG=\"${CARGO_PROFILE_TEST_DEBUG:-0}\"\n");
            }
            action.push_str(&format!("cargo {command}{locked}"));
            actions.push_str(&format!(
                "\n[[actions]]\nname = {}\nicon = {}\ncommand = {}\n",
                quote(name),
                quote(icon),
                quote(&action)
            ));
        }
    }
    if codegraph_enabled(root) {
        if !setup.contains(ROOTS) {
            setup.push_str(ROOTS);
        }
        setup.push_str(CODEGRAPH_SETUP);
    }
    Ok(migration::managed(&format!(
        "version = 1\nname = {}\n\n[setup]\nscript = '''\n{}'''\n{actions}",
        quote(name),
        setup
    )))
}

fn codegraph_enabled(root: &Path) -> bool {
    root.join(".codegraph/codegraph.db").is_file()
        || std::process::Command::new("git")
            .args(["worktree", "list", "--porcelain", "-z"])
            .current_dir(root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|output| {
                output
                    .split('\0')
                    .next()?
                    .strip_prefix("worktree ")
                    .map(str::to_owned)
            })
            .is_some_and(|primary| {
                Path::new(&primary)
                    .join(".codegraph/codegraph.db")
                    .is_file()
            })
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

#[cfg(all(test, unix))]
mod codegraph_tests;

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
        assert!(!content.contains("codegraph sync"));
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
        assert!(script.contains(".backup snapshot.db"));
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
        assert!(render(&worktree).unwrap().contains("codegraph sync"));
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
        assert!(primary.join(".git/jt-codex/codegraph.db").is_file());
        assert!(!worktree.join(".codegraph/daemon.pid").exists());
        assert!(
            Command::new("sh")
                .args(["-c", script])
                .env(
                    "PATH",
                    std::env::join_paths(
                        std::iter::once(directory.path().join("bin"))
                            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))
                    )
                    .unwrap()
                )
                .current_dir(&worktree)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            copy.query_row("SELECT value FROM evidence", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "worktree"
        );
    }

    #[test]
    fn cargo_cache_and_lightweight_variables_apply_to_actual_actions_only() {
        let project = tempdir().unwrap();
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(project.path().join("Cargo.lock"), "").unwrap();
        let content = render(project.path()).unwrap();
        assert!(content.contains("cargo test --locked"));
        assert!(content.contains("CARGO_TARGET_DIR"));
        assert!(!content.contains("CARGO_INCREMENTAL"));
        let light = render_with_options(project.path(), true).unwrap();
        assert!(light.contains("CARGO_PROFILE_TEST_DEBUG"));
        let setup = light
            .split_once("script = '''\n")
            .unwrap()
            .1
            .split_once("'''")
            .unwrap()
            .0;
        assert!(!setup.contains("CARGO_"));
        fs::remove_file(project.path().join("Cargo.toml")).unwrap();
        assert!(
            !render_with_options(project.path(), true)
                .unwrap()
                .contains("CARGO_")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cargo_test_action_receives_cache_and_preserves_user_environment() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let project = tempdir().unwrap();
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(project.path().join("Cargo.lock"), "").unwrap();
        let bin = project.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("cargo"), "#!/bin/sh\nprintf '%s\\n' \"$CARGO_TARGET_DIR\" \"$CARGO_PROFILE_TEST_DEBUG\" \"$*\" > cargo-evidence\n").unwrap();
        fs::set_permissions(bin.join("cargo"), fs::Permissions::from_mode(0o755)).unwrap();
        let content = render_with_options(project.path(), true).unwrap();
        let action: String = serde_json::from_str(
            content
                .split_once("name = \"Rust tests\"")
                .unwrap()
                .1
                .split_once("command = ")
                .unwrap()
                .1
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let mut command = Command::new("sh");
        command
            .args(["-c", &action])
            .env("PATH", path)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_PROFILE_TEST_DEBUG")
            .current_dir(project.path());
        assert!(command.status().unwrap().success());
        let evidence = fs::read_to_string(project.path().join("cargo-evidence")).unwrap();
        assert!(
            evidence.ends_with("/jt-codex/cargo-target\n0\ntest --locked\n"),
            "{evidence}"
        );
        assert!(
            command
                .env("CARGO_TARGET_DIR", "/custom/cache")
                .env("CARGO_PROFILE_TEST_DEBUG", "1")
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(project.path().join("cargo-evidence")).unwrap(),
            "/custom/cache\n1\ntest --locked\n"
        );
    }
}
