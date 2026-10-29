use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use tempfile::tempdir;

fn jt(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jt"))
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap()
}

fn successful(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git(root: &Path, arguments: &[&str]) -> String {
    successful(
        Command::new("git")
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(arguments)
            .env("GIT_AUTHOR_NAME", "JT tests")
            .env("GIT_AUTHOR_EMAIL", "jt-tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "JT tests")
            .env("GIT_COMMITTER_EMAIL", "jt-tests@example.invalid")
            .current_dir(root)
            .output()
            .unwrap(),
    )
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn repository(root: &Path) {
    git(root, &["init", "-q"]);
    write(root, "README.md", "fixture\n");
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-qm", "fixture"]);
}

#[cfg(unix)]
#[test]
fn init_does_not_install_simple_hooks_into_a_different_effective_hooks_directory() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().unwrap();
    repository(root.path());
    write(root.path(), "package.json", r#"{"simple-git-hooks":{}}"#);
    let original = "#!/bin/sh\nexit 37\n";
    write(root.path(), ".git/hooks/post-checkout", original);
    write(
        root.path(),
        "node_modules/.bin/simple-git-hooks",
        "#!/bin/sh\ntouch installer-ran\n",
    );
    fs::set_permissions(
        root.path().join("node_modules/.bin/simple-git-hooks"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let global = root.path().join("global.gitconfig");
    fs::write(&global, "[core]\n hooksPath = effective-hooks\n").unwrap();
    let output = successful(
        Command::new(env!("CARGO_BIN_EXE_jt"))
            .args(["codex", "init"])
            .env("GIT_CONFIG_GLOBAL", &global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(root.path())
            .output()
            .unwrap(),
    );
    assert!(output.contains("installation path differs"), "{output}");
    assert!(!root.path().join("installer-ran").exists());
    assert!(!root.path().join("effective-hooks/post-checkout").exists());
    assert_eq!(
        fs::read_to_string(root.path().join(".git/hooks/post-checkout")).unwrap(),
        original
    );
    assert_eq!(
        fs::read_to_string(&global).unwrap(),
        "[core]\n hooksPath = effective-hooks\n"
    );
}

fn linked_worktree(root: &Path, target: &Path) {
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            target.to_str().unwrap(),
            "HEAD",
        ],
    );
}

#[test]
fn init_without_hook_manager_preserves_custom_manifest_and_reports_disabled() {
    let root = tempdir().unwrap();
    repository(root.path());
    write(
        root.path(),
        ".worktreeinclude",
        "# project choice\nprivate.ini\n",
    );

    let first = successful(jt(root.path(), &["codex", "init"]));
    assert!(first.contains("原生 Git 自动继承尚未启用"), "{first}");
    let manifest = fs::read_to_string(root.path().join(".worktreeinclude")).unwrap();
    assert!(manifest.contains("# project choice\nprivate.ini\n"));
    for pattern in [
        ".env.local",
        ".env.*.local",
        "CLAUDE.local.md",
        "AGENTS.override.md",
    ] {
        assert!(manifest.contains(pattern), "missing {pattern}: {manifest}");
    }
    assert!(
        root.path()
            .join(".codex/environments/environment.toml")
            .is_file()
    );
    successful(jt(root.path(), &["codex", "init"]));
    assert_eq!(
        fs::read_to_string(root.path().join(".worktreeinclude")).unwrap(),
        manifest
    );
    assert!(!root.path().join("package.json").exists());
    assert!(!root.path().join("lefthook.yml").exists());
    assert!(!root.path().join(".simple-git-hooks.json").exists());
    assert!(!root.path().join(".git/hooks/post-checkout").exists());
}

#[test]
fn init_check_does_not_change_manifest_or_hook_configuration() {
    let root = tempdir().unwrap();
    repository(root.path());
    let hook = r#"{"post-checkout":"printf '%s\\n' \"$@\""}"#;
    write(root.path(), ".simple-git-hooks.json", hook);
    write(root.path(), ".worktreeinclude", "private.ini\n");
    write(
        root.path(),
        ".codex/environments/environment.toml",
        "custom environment\n",
    );

    let output = successful(jt(root.path(), &["codex", "init", "--check"]));
    assert!(output.contains("candidate:"), "{output}");
    assert_eq!(
        fs::read_to_string(root.path().join(".simple-git-hooks.json")).unwrap(),
        hook
    );
    assert_eq!(
        fs::read_to_string(root.path().join(".worktreeinclude")).unwrap(),
        "private.ini\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join(".codex/environments/environment.toml")).unwrap(),
        "custom environment\n"
    );
    assert!(!root.path().join(".git/hooks/post-checkout").exists());
}

#[test]
fn explicit_inherit_fills_missing_configuration_once_and_preserves_target_values() {
    let fixture = tempdir().unwrap();
    let primary = fixture.path().join("primary");
    let target = fixture.path().join("target");
    fs::create_dir(&primary).unwrap();
    repository(&primary);
    write(&primary, ".worktreeinclude", ".config/private.ini\n");
    successful(jt(&primary, &["codex", "init"]));
    write(&primary, ".env.local", "TOKEN=source\n");
    write(&primary, ".env.test.local", "TOKEN=test\n");
    write(&primary, ".nlab/cli.local.json", "{\"source\":true}\n");
    write(&primary, "CLAUDE.local.md", "source instructions\n");
    write(&primary, "AGENTS.override.md", "source override\n");
    write(&primary, ".config/private.ini", "private=true\n");
    write(&primary, ".codex/reports/private.log", "must stay local\n");
    git(
        &primary,
        &[
            "add",
            ".worktreeinclude",
            ".codex/environments/environment.toml",
        ],
    );
    git(&primary, &["commit", "-qm", "configuration"]);
    linked_worktree(&primary, &target);
    write(&target, ".env.local", "TOKEN=target\n");

    successful(jt(&target, &["codex", "inherit"]));
    assert_eq!(
        fs::read_to_string(target.join(".env.local")).unwrap(),
        "TOKEN=target\n"
    );
    assert_eq!(
        fs::read_to_string(target.join(".env.test.local")).unwrap(),
        "TOKEN=test\n"
    );
    assert_eq!(
        fs::read_to_string(target.join(".nlab/cli.local.json")).unwrap(),
        "{\"source\":true}\n"
    );
    assert_eq!(
        fs::read_to_string(target.join("CLAUDE.local.md")).unwrap(),
        "source instructions\n"
    );
    assert_eq!(
        fs::read_to_string(target.join("AGENTS.override.md")).unwrap(),
        "source override\n"
    );
    assert_eq!(
        fs::read_to_string(target.join(".config/private.ini")).unwrap(),
        "private=true\n"
    );
    assert!(!target.join(".codex/reports").exists());

    write(&primary, ".env.test.local", "TOKEN=source-later\n");
    successful(jt(&target, &["codex", "inherit"]));
    assert_eq!(
        fs::read_to_string(target.join(".env.test.local")).unwrap(),
        "TOKEN=test\n"
    );
}

#[test]
fn checkout_inheritance_ignores_branch_switches_and_primary_checkout() {
    let fixture = tempdir().unwrap();
    let primary = fixture.path().join("primary");
    let target = fixture.path().join("target");
    fs::create_dir(&primary).unwrap();
    repository(&primary);
    successful(jt(&primary, &["codex", "init"]));
    write(&primary, ".env.local", "TOKEN=source\n");
    git(&primary, &["add", ".worktreeinclude"]);
    git(&primary, &["commit", "-qm", "manifest"]);
    linked_worktree(&primary, &target);
    let head = git(&primary, &["rev-parse", "HEAD"]);
    let head = head.trim();
    let zero = "0000000000000000000000000000000000000000";

    for (old, flag) in [(head, "1"), (zero, "0")] {
        successful(jt(
            &target,
            &["codex", "inherit", "--post-checkout", old, head, flag],
        ));
        assert!(!target.join(".env.local").exists());
    }
    fs::remove_file(primary.join(".env.local")).unwrap();
    write(&target, ".env.local", "TOKEN=target\n");
    successful(jt(
        &primary,
        &["codex", "inherit", "--post-checkout", zero, head, "1"],
    ));
    assert!(!primary.join(".env.local").exists());
    fs::remove_file(target.join(".env.local")).unwrap();
    write(&primary, ".env.local", "TOKEN=source\n");
    successful(jt(
        &target,
        &["codex", "inherit", "--post-checkout", zero, head, "1"],
    ));
    assert_eq!(
        fs::read_to_string(target.join(".env.local")).unwrap(),
        "TOKEN=source\n"
    );
}

#[cfg(unix)]
#[test]
fn generated_codex_setup_inherits_missing_configuration() {
    let fixture = tempdir().unwrap();
    let primary = fixture.path().join("primary");
    let target = fixture.path().join("target");
    fs::create_dir(&primary).unwrap();
    repository(&primary);
    successful(jt(&primary, &["codex", "init"]));
    write(&primary, ".env.local", "TOKEN=setup\n");
    git(
        &primary,
        &[
            "add",
            ".worktreeinclude",
            ".codex/environments/environment.toml",
        ],
    );
    git(&primary, &["commit", "-qm", "configuration"]);
    linked_worktree(&primary, &target);
    let environment =
        fs::read_to_string(target.join(".codex/environments/environment.toml")).unwrap();
    let script = environment
        .split_once("script = '''\n")
        .unwrap()
        .1
        .split_once("'''\n")
        .unwrap()
        .0;
    let binary_directory = Path::new(env!("CARGO_BIN_EXE_jt")).parent().unwrap();
    let path = std::env::join_paths(std::iter::once(binary_directory.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();

    successful(
        Command::new("sh")
            .args(["-c", script])
            .env("PATH", path)
            .current_dir(&target)
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(target.join(".env.local")).unwrap(),
        "TOKEN=setup\n"
    );
}

#[test]
fn generated_node_setup_inherits_before_dependency_preparation() {
    let root = tempdir().unwrap();
    repository(root.path());
    write(root.path(), "package.json", "{\"private\":true}\n");
    successful(jt(root.path(), &["codex", "init"]));
    let environment =
        fs::read_to_string(root.path().join(".codex/environments/environment.toml")).unwrap();
    assert!(
        environment.find("codex inherit").unwrap()
            < environment.find("node --input-type=module").unwrap()
    );
    assert!(!root.path().join("node_modules").exists());
}

#[test]
fn explicit_inherit_uses_primary_manifest_for_no_checkout_worktree() {
    let fixture = tempdir().unwrap();
    let primary = fixture.path().join("primary");
    let target = fixture.path().join("target");
    fs::create_dir(&primary).unwrap();
    repository(&primary);
    successful(jt(&primary, &["codex", "init"]));
    write(&primary, ".env.local", "TOKEN=no-checkout\n");
    git(&primary, &["add", ".worktreeinclude"]);
    git(&primary, &["commit", "-qm", "manifest"]);
    git(
        &primary,
        &[
            "worktree",
            "add",
            "--detach",
            "--no-checkout",
            target.to_str().unwrap(),
            "HEAD",
        ],
    );
    assert!(!target.join(".worktreeinclude").exists());
    successful(jt(&target, &["codex", "inherit"]));
    assert_eq!(
        fs::read_to_string(target.join(".env.local")).unwrap(),
        "TOKEN=no-checkout\n"
    );
}
