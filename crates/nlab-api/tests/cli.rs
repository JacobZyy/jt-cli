use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn nlab_api() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nlab-api"))
}

#[test]
fn exposes_init_and_generate_only() {
    let output = nlab_api().arg("--help").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains("init"));
    assert!(stdout.contains("generate"));
    for hidden in ["routes", "migrate", "mock", "accept"] {
        assert!(!stdout.contains(hidden));
    }
}

#[test]
fn invalid_generate_project_is_non_mutating() {
    let project = tempdir().unwrap();

    let output = nlab_api()
        .args(["generate", "--project", project.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("read nlab-api config"));
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 0);
}
