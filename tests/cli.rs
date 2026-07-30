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
    assert!(!stdout.contains("jt release init"));
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
}
