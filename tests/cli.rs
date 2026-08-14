use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use tempfile::tempdir;

fn jt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jt"))
}

fn init_git(path: &Path) {
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .unwrap()
            .success()
    );
}

fn write_vitest_package(path: &Path) {
    fs::write(
        path.join("package.json"),
        r#"{"devDependencies":{"tsx":"^4.0.0","vitest":"^4.0.0"}}"#,
    )
    .unwrap();
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
    assert!(stdout.contains("jt vitest ai-hook --codex"));
    assert!(stdout.contains("jt vitest ai-hook --claude"));
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
fn vitest_ai_hook_rejects_invalid_argument_shapes_without_mutation() {
    let project = tempdir().unwrap();
    for args in [
        &["vitest", "ai-hook"][..],
        &["vitest", "ai-hook", "--unknown"][..],
        &["vitest", "ai-hook", "--codex", "--claude"][..],
    ] {
        let output = jt()
            .args(args)
            .current_dir(project.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn vitest_ai_hook_requires_git_without_mutation() {
    let project = tempdir().unwrap();
    write_vitest_package(project.path());

    let output = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not inside a Git repository"));
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn vitest_ai_hook_requires_root_dependency_without_mutation() {
    let project = tempdir().unwrap();
    init_git(project.path());
    fs::write(
        project.path().join("package.json"),
        r#"{"devDependencies":{"vite":"^7.0.0"}}"#,
    )
    .unwrap();

    let output = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not declare vitest"));
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn vitest_ai_hook_requires_tsx_without_mutation() {
    let project = tempdir().unwrap();
    init_git(project.path());
    fs::write(
        project.path().join("package.json"),
        r#"{"devDependencies":{"vitest":"^4.0.0"}}"#,
    )
    .unwrap();

    let output = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not declare tsx"));
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn vitest_ai_hook_defers_claude_without_mutation() {
    let project = tempdir().unwrap();

    let output = jt()
        .args(["vitest", "ai-hook", "--claude"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Claude support is not implemented"));
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn vitest_ai_hook_install_upgrades_owned_files_preserves_hooks_and_is_byte_stable() {
    let project = tempdir().unwrap();
    init_git(project.path());
    write_vitest_package(project.path());
    let nested = project.path().join("packages/demo");
    fs::create_dir_all(&nested).unwrap();
    let hook_dir = project.path().join(".codex/hooks/jt-vitest");
    fs::create_dir_all(&hook_dir).unwrap();
    fs::write(
        hook_dir.join("vitest.ts"),
        "// jt-vitest-ai-hook\n// stale owned template\n",
    )
    .unwrap();
    fs::write(
        project.path().join(".codex/hooks.json"),
        r#"{
  "project": {"keep": true},
  "hooks": {
    "Start": [{"hooks": [{"type": "command", "command": "echo start"}]}],
    "PreToolUse": [
      {"matcher": "keep", "hooks": [{"type": "command", "command": "echo pre"}]},
      {"matcher": "old", "hooks": [{"type": "command", "command": "pnpm exec tsx .codex/hooks/jt-vitest/pre-tool-use.ts codex", "timeout": 1}]}
    ],
    "PostToolUse": [
      {"matcher": "old", "hooks": [{"type": "command", "command": "pnpm exec tsx .codex/hooks/jt-vitest/post-tool-use.ts codex", "timeout": 1}]}
    ],
    "Stop": [
      {"matcher": "keep", "hooks": [{"type": "command", "command": "echo stop"}]},
      {"hooks": [{"type": "command", "command": "pnpm exec tsx .codex/hooks/jt-vitest/stop.ts codex", "timeout": 1}]}
    ]
  }
}
"#,
    )
    .unwrap();

    let first = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(&nested)
        .output()
        .unwrap();
    assert!(first.status.success());
    assert!(String::from_utf8_lossy(&first.stdout).contains("/hooks"));

    let path = project.path().join(".codex/hooks.json");
    let first_bytes = fs::read(&path).unwrap();
    let installed_text = String::from_utf8(first_bytes.clone()).unwrap();
    assert!(
        installed_text.find("\"project\"").unwrap() < installed_text.find("\"hooks\"").unwrap()
    );
    assert!(installed_text.contains("\"matcher\": \"keep\",\n        \"hooks\""));
    assert!(
        installed_text.contains("\"type\": \"command\",\n            \"command\": \"echo start\"")
    );
    let installed: serde_json::Value = serde_json::from_slice(&first_bytes).unwrap();
    assert_eq!(installed["project"]["keep"], true);
    assert_eq!(
        installed["hooks"]["Start"][0]["hooks"][0]["command"],
        "echo start"
    );
    let pre = installed["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2);
    assert_eq!(pre[0]["hooks"][0]["command"], "echo pre");
    assert_eq!(
        installed["hooks"]["PostToolUse"].as_array().unwrap().len(),
        1
    );
    let stop = installed["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop[0]["matcher"], "keep");
    assert_eq!(stop[0]["hooks"][0]["command"], "echo stop");
    let owned = &stop[1]["hooks"][0];
    assert_eq!(
        owned,
        &serde_json::json!({
            "command": "pnpm --dir \"$(git rev-parse --show-toplevel)\" exec tsx .codex/hooks/jt-vitest/stop.ts codex",
            "statusMessage": "Running related Vitest suites",
            "timeout": 150,
            "type": "command"
        })
    );
    for (event, file, matcher) in [
        ("PreToolUse", "pre-tool-use.ts", Some("Edit|Write")),
        ("PostToolUse", "post-tool-use.ts", Some("Edit|Write")),
        ("Stop", "stop.ts", None),
    ] {
        let group = installed["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .find(|group| {
                group["hooks"][0]["command"]
                    .as_str()
                    .is_some_and(|command| {
                        command.contains(&format!(".codex/hooks/jt-vitest/{file}"))
                    })
            })
            .unwrap();
        assert_eq!(
            group.get("matcher").and_then(|value| value.as_str()),
            matcher
        );
    }
    for file in [
        "coverage.ts",
        "files.ts",
        "post-tool-use.ts",
        "pre-tool-use.ts",
        "protocol.ts",
        "runtime.ts",
        "stop.ts",
        "vitest.ts",
    ] {
        let source =
            fs::read_to_string(project.path().join(".codex/hooks/jt-vitest").join(file)).unwrap();
        assert!(source.contains("jt-vitest-ai-hook"));
    }
    let vitest_source = fs::read_to_string(hook_dir.join("vitest.ts")).unwrap();
    for expected in [
        "--reporter=agent",
        "--coverage.enabled",
        "--coverage.enabled=false",
        "--coverage.include=",
        "--coverage.reporter=text",
        "selectCoverageFiles",
    ] {
        assert!(vitest_source.contains(expected), "missing {expected}");
    }
    assert!(!vitest_source.contains("--coverage.skipFull"));
    assert!(!vitest_source.contains("stale owned template"));
    let coverage_source = fs::read_to_string(hook_dir.join("coverage.ts")).unwrap();
    for expected in [
        "resolveConfig",
        "coverage.include",
        "coverage.exclude",
        "picomatch",
    ] {
        assert!(coverage_source.contains(expected), "missing {expected}");
    }
    let stop_source = fs::read_to_string(hook_dir.join("stop.ts")).unwrap();
    assert!(stop_source.contains("output: detail,"));
    assert!(stop_source.contains("${coverage.warning} Log: ${runtime.logPath}"));
    for unexpected in [
        "Related Vitest suites and coverage passed.",
        "Related Vitest suites passed; no AI-edited files matched project coverage rules.",
    ] {
        assert!(!stop_source.contains(unexpected), "unexpected {unexpected}");
    }

    let second = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(&nested)
        .output()
        .unwrap();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("already configured"));
    assert_eq!(fs::read(path).unwrap(), first_bytes);
}

#[test]
fn vitest_ai_hook_refuses_to_replace_unowned_template() {
    let project = tempdir().unwrap();
    init_git(project.path());
    write_vitest_package(project.path());
    let hook_dir = project.path().join(".codex/hooks/jt-vitest");
    fs::create_dir_all(&hook_dir).unwrap();
    let path = hook_dir.join("stop.ts");
    fs::write(&path, "custom stop hook\n").unwrap();

    let output = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unowned"));
    assert_eq!(fs::read_to_string(path).unwrap(), "custom stop hook\n");
    assert!(!project.path().join(".codex/hooks.json").exists());
}

#[test]
fn vitest_ai_hook_rejects_invalid_existing_json_without_mutation() {
    let project = tempdir().unwrap();
    init_git(project.path());
    write_vitest_package(project.path());
    fs::create_dir(project.path().join(".codex")).unwrap();
    let path = project.path().join(".codex/hooks.json");
    let original = b"{not json\n";
    fs::write(&path, original).unwrap();

    let output = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid"));
    assert_eq!(fs::read(path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn vitest_ai_hook_rejects_symlinked_file_without_mutating_target() {
    use std::os::unix::fs::symlink;

    let project = tempdir().unwrap();
    let external = tempdir().unwrap();
    init_git(project.path());
    write_vitest_package(project.path());
    fs::create_dir(project.path().join(".codex")).unwrap();
    let target = external.path().join("hooks.json");
    let original = b"{\"external\":true}\n";
    fs::write(&target, original).unwrap();
    let link = project.path().join(".codex/hooks.json");
    symlink(&target, &link).unwrap();

    let output = jt()
        .args(["vitest", "ai-hook", "--codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlinked file"));
    assert_eq!(fs::read(target).unwrap(), original);
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
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
    init_git(project.path());
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
