use std::process::{Command, Stdio};

use tempfile::tempdir;

fn jt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jt"))
}

#[test]
fn help_lists_new_commands_only() {
    let output = jt().arg("--help").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains("jt repo cicd"));
    assert!(stdout.contains("jt node init"));
    assert!(stdout.contains("jt cli bootstrap"));
    assert!(stdout.contains("jt ghostty install"));
    assert!(stdout.contains("jt zed-conf"));
    assert!(stdout.contains("jt upgrade"));
    assert!(stdout.contains("completions"));
    assert!(
        !stdout
            .lines()
            .any(|line| line.trim_start().starts_with("help "))
    );
    assert!(!stdout.contains("jt release init"));
}

#[test]
fn no_arguments_prints_help_and_exits_two() {
    let output = jt().output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    let output_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output_text.contains("Usage: jt <COMMAND>"));
}

#[test]
fn version_reports_cargo_package_version() {
    let output = jt().arg("--version").output().unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("jt {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn completions_generate_fish_and_zsh_scripts() {
    for (shell, marker) in [("fish", "complete -c jt"), ("zsh", "#compdef jt")] {
        let output = jt().args(["completions", shell]).output().unwrap();

        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(String::from_utf8(output.stdout).unwrap().contains(marker));
    }
}

#[test]
fn completions_reject_unknown_shell() {
    let output = jt().args(["completions", "bash"]).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn upgrade_help_and_invalid_version_do_not_touch_network() {
    let help = jt().args(["upgrade", "--help"]).output().unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("--dry-run")
    );

    let invalid = jt()
        .args(["upgrade", "latest && rm -rf /"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid jt version"));

    for flag in ["--check", "--dry-run", "--force"] {
        let flag_shaped_version = jt()
            .args(["upgrade", "--", flag])
            .env("PATH", "")
            .output()
            .unwrap();
        assert_eq!(flag_shaped_version.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&flag_shaped_version.stderr).contains("invalid jt version")
        );
    }
}

#[test]
fn old_release_command_is_rejected() {
    let output = jt().args(["release", "init"]).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn node_init_requires_tty_before_mutating_home() {
    let home = tempdir().unwrap();
    let output = jt()
        .args(["node", "init"])
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(!home.path().join(".vite-plus").exists());
}

#[test]
fn cli_bootstrap_requires_tty_before_mutating_home() {
    let home = tempdir().unwrap();
    let output = jt()
        .args(["cli", "bootstrap"])
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(home.path().read_dir().unwrap().next().is_none());
}

#[test]
fn ghostty_install_rejects_server_or_requires_tty_before_mutating_home() {
    let home = tempdir().unwrap();
    let output = jt()
        .args(["ghostty", "install"])
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(home.path().read_dir().unwrap().next().is_none());
    if cfg!(target_os = "linux") {
        assert!(String::from_utf8_lossy(&output.stderr).contains("仅支持 macOS"));
    }
}

#[test]
fn repo_cicd_dispatches_to_release_initializer() {
    let project = tempdir().unwrap();
    std::fs::write(
        project.path().join("package.json"),
        r#"{
  "name": "@acme/demo",
  "version": "1.0.0",
  "repository": "https://github.com/acme/demo.git",
  "scripts": {
    "test": "node --test",
    "build": "node build.js"
  }
}"#,
    )
    .unwrap();
    std::fs::write(project.path().join("package-lock.json"), "{}\n").unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(project.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/acme/demo.git",
            ])
            .current_dir(project.path())
            .status()
            .unwrap()
            .success()
    );

    let output = jt()
        .args(["repo", "cicd"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(
        project
            .path()
            .join(".github/workflows/npm-release.yml")
            .is_file()
    );
    assert!(project.path().join("release-please-config.json").is_file());
    assert!(
        project
            .path()
            .join(".release-please-manifest.json")
            .is_file()
    );
}

#[test]
fn repo_cicd_missing_origin_requires_tty_without_mutation() {
    let project = tempdir().unwrap();
    std::fs::write(
        project.path().join("package.json"),
        r#"{
  "name": "@acme/demo",
  "version": "1.0.0",
  "repository": "https://github.com/acme/demo.git"
}"#,
    )
    .unwrap();

    let output = jt()
        .args(["repo", "cicd"])
        .current_dir(project.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!project.path().join(".git").exists());
    assert!(!project.path().join(".github").exists());
    assert!(!project.path().join("release-please-config.json").exists());
    assert!(
        !project
            .path()
            .join(".release-please-manifest.json")
            .exists()
    );
}
