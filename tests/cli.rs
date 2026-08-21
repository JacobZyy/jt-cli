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

fn write_ai_hook_package(path: &Path) {
    fs::write(
        path.join("package.json"),
        r#"{"devDependencies":{"eslint":"^9.0.0","tsx":"^4.0.0","vitest":"^4.0.0"}}"#,
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
    assert!(stdout.contains("jt nlab-api generate --help"));
    assert!(stdout.contains("jt cli bootstrap"));
    assert!(stdout.contains("jt ghostty install"));
    assert!(stdout.contains("jt zed-conf"));
    assert!(stdout.contains("jt ai-hook"));
    assert!(stdout.contains("jt ai-hook --checks vitest,eslint --agents codex"));
    assert!(stdout.contains("jt vitest"));
    assert!(!stdout.contains("jt vitest ai-hook"));
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
fn nlab_api_help_and_invalid_repo_are_non_mutating() {
    let group_help = jt().args(["nlab-api", "--help"]).output().unwrap();
    assert!(group_help.status.success());
    let group_help = String::from_utf8(group_help.stdout).unwrap();
    for command in ["init", "generate", "routes", "migrate", "mock", "accept"] {
        assert!(group_help.contains(command), "missing nlab-api {command}");
    }

    let init_help = jt().args(["nlab-api", "init", "--help"]).output().unwrap();
    assert!(init_help.status.success());
    let init_help = String::from_utf8(init_help.stdout).unwrap();
    assert!(init_help.contains("--project"));
    assert!(init_help.contains("--repo-path"));
    assert!(init_help.contains("--layout"));

    let help = jt()
        .args(["nlab-api", "generate", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--project"));
    assert!(help.contains("--timeout-seconds"));

    let root = tempdir().unwrap();
    let result = jt()
        .args([
            "nlab-api",
            "generate",
            "--project",
            root.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("read nlab-api config"));
    assert!(!root.path().join(".nlab").exists());
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
fn ai_hook_rejects_invalid_argument_shapes_and_old_command_without_mutation() {
    let project = tempdir().unwrap();
    for args in [
        &["ai-hook", "--checks", "vitest"][..],
        &["ai-hook", "--agents", "codex"][..],
        &["ai-hook", "--unknown"][..],
        &["vitest", "ai-hook", "--codex"][..],
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
fn ai_hook_requires_git_without_mutation() {
    let project = tempdir().unwrap();
    write_ai_hook_package(project.path());

    let output = jt()
        .args(["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not inside a Git repository"));
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn ai_hook_requires_selected_dependency_without_mutation() {
    let project = tempdir().unwrap();
    init_git(project.path());
    fs::write(
        project.path().join("package.json"),
        r#"{"devDependencies":{"tsx":"^4.0.0"}}"#,
    )
    .unwrap();

    let output = jt()
        .args(["ai-hook", "--checks", "vitest", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not declare vitest"));
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn ai_hook_requires_tsx_without_mutation() {
    let project = tempdir().unwrap();
    init_git(project.path());
    fs::write(
        project.path().join("package.json"),
        r#"{"devDependencies":{"vitest":"^4.0.0"}}"#,
    )
    .unwrap();

    let output = jt()
        .args(["ai-hook", "--checks", "vitest", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not declare tsx"));
    assert!(!project.path().join(".codex").exists());
}

#[test]
fn ai_hook_requires_tty_without_flags_and_vitest_is_placeholder() {
    let project = tempdir().unwrap();
    init_git(project.path());

    let output = jt()
        .arg("ai-hook")
        .current_dir(project.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal required"));
    assert!(!project.path().join(".codex").exists());

    let vitest = jt().arg("vitest").output().unwrap();
    assert!(vitest.status.success());
    assert!(String::from_utf8_lossy(&vitest.stdout).contains("not implemented yet"));
}

#[test]
fn ai_hook_install_migrates_old_hooks_preserves_unrelated_hooks_and_is_byte_stable() {
    let project = tempdir().unwrap();
    init_git(project.path());
    write_ai_hook_package(project.path());
    let nested = project.path().join("packages/demo");
    fs::create_dir_all(&nested).unwrap();
    let hook_dir = project.path().join(".codex/hooks/jt-ai-hook");
    fs::create_dir_all(hook_dir.join("stop/runner")).unwrap();
    fs::write(
        hook_dir.join("stop/runner/vitest.ts"),
        "// jt-ai-hook\n// stale owned template\n",
    )
    .unwrap();
    let old_vitest_dir = project.path().join(".codex/hooks/jt-vitest");
    fs::create_dir_all(&old_vitest_dir).unwrap();
    fs::write(old_vitest_dir.join("stop.ts"), "// jt-vitest-ai-hook\n").unwrap();
    fs::write(old_vitest_dir.join("runtime.ts"), "// custom runtime\n").unwrap();
    fs::write(old_vitest_dir.join("custom.ts"), "// custom hook\n").unwrap();
    let old_eslint_dir = project.path().join(".codex/hooks/nlab-eslint");
    fs::create_dir_all(&old_eslint_dir).unwrap();
    fs::write(old_eslint_dir.join("stop.ts"), "// nlab-eslint-ai-hook\n").unwrap();
    let old_eslint_leaf = project.path().join(".codex/hooks/nlab-eslint-stop.mjs");
    fs::write(&old_eslint_leaf, "// nlab-eslint-ai-hook\n").unwrap();
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
      {"matcher": "old", "hooks": [
        {"type": "command", "command": "pnpm exec tsx .codex/hooks/jt-vitest/post-tool-use.ts codex", "timeout": 1},
        {"type": "command", "command": "echo post-keep"}
      ]}
    ],
    "Stop": [
      {"matcher": "keep", "hooks": [
        {"type": "command", "command": "echo stop"},
        {"type": "command", "command": "pnpm exec tsx .codex/hooks/jt-ai-hook/custom.ts codex"}
      ]},
      {"hooks": [{"type": "command", "command": "pnpm exec tsx .codex/hooks/jt-vitest/stop.ts codex", "timeout": 1}]},
      {"hooks": [{"type": "command", "command": "pnpm exec tsx .codex/hooks/nlab-eslint/stop.ts codex", "timeout": 1}]}
    ]
  }
}
"#,
    )
    .unwrap();

    let first = jt()
        .args(["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"])
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
    assert!(!installed_text.contains(".codex/hooks/jt-vitest/"));
    assert!(!installed_text.contains(".codex/hooks/nlab-eslint/"));
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
        2
    );
    assert_eq!(
        installed["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "echo post-keep"
    );
    let stop = installed["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop[0]["matcher"], "keep");
    assert_eq!(stop[0]["hooks"][0]["command"], "echo stop");
    assert_eq!(
        stop[0]["hooks"][1]["command"],
        "pnpm exec tsx .codex/hooks/jt-ai-hook/custom.ts codex"
    );
    let owned = &stop[1]["hooks"][0];
    assert_eq!(
        owned,
        &serde_json::json!({
            "command": "pnpm --dir \"$(git rev-parse --show-toplevel)\" exec tsx .codex/hooks/jt-ai-hook/stop-entry.ts codex",
            "statusMessage": "Running AI checks",
            "timeout": 150,
            "type": "command"
        })
    );
    for (event, file, matcher) in [
        ("PreToolUse", "pre-tool-use.ts", Some("Edit|Write")),
        ("PostToolUse", "post-tool-use.ts", Some("Edit|Write")),
        ("Stop", "stop-entry.ts", None),
    ] {
        let group = installed["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .find(|group| {
                group["hooks"][0]["command"]
                    .as_str()
                    .is_some_and(|command| {
                        command.contains(&format!(".codex/hooks/jt-ai-hook/{file}"))
                    })
            })
            .unwrap();
        assert_eq!(
            group.get("matcher").and_then(|value| value.as_str()),
            matcher
        );
    }
    for file in [
        "files.ts",
        "post-tool-use.ts",
        "pre-tool-use.ts",
        "protocol.ts",
        "runtime.ts",
        "stop-entry.ts",
        "stop/process.ts",
        "stop/types.ts",
        "stop/support/vitest-coverage.ts",
        "stop/runner/eslint.ts",
        "stop/runner/vitest.ts",
    ] {
        let source = fs::read_to_string(hook_dir.join(file)).unwrap();
        assert!(source.contains("jt-ai-hook"));
    }
    let vitest_source = fs::read_to_string(hook_dir.join("stop/runner/vitest.ts")).unwrap();
    for expected in [
        "--reporter=agent",
        "--coverage.enabled",
        "--coverage.enabled=false",
        "--coverage.include=",
        "--coverage.reporter=json-summary",
        "--coverage.reportsDirectory=",
        "--coverage.reportOnFailure",
        "coverage-summary.json",
        "| File | Statements | Branches | Functions | Lines |",
        "formatFailureReport",
        "isCoverageOnlyFailure",
        "All files ${metric}",
        "selectCoverageFiles",
    ] {
        assert!(vitest_source.contains(expected), "missing {expected}");
    }
    assert!(!vitest_source.contains("--coverage.reporter=text"));
    assert!(!vitest_source.contains("--coverage.skipFull"));
    assert!(!vitest_source.contains("stale owned template"));
    assert!(vitest_source.contains("isInAIHook: 'true'"));
    assert!(!old_vitest_dir.join("stop.ts").exists());
    assert_eq!(
        fs::read_to_string(old_vitest_dir.join("runtime.ts")).unwrap(),
        "// custom runtime\n"
    );
    assert_eq!(
        fs::read_to_string(old_vitest_dir.join("custom.ts")).unwrap(),
        "// custom hook\n"
    );
    assert!(!old_eslint_dir.exists());
    assert!(!old_eslint_leaf.exists());
    let coverage_source =
        fs::read_to_string(hook_dir.join("stop/support/vitest-coverage.ts")).unwrap();
    for expected in [
        "resolveConfig",
        "coverage.include",
        "coverage.exclude",
        "coverage.skipFull",
        "picomatch",
    ] {
        assert!(coverage_source.contains(expected), "missing {expected}");
    }
    let stop_source = fs::read_to_string(hook_dir.join("stop-entry.ts")).unwrap();
    assert!(stop_source.contains("Promise.allSettled"));
    assert!(stop_source.contains("stop/runner"));
    assert!(stop_source.contains("entry.isFile()"));
    let process_source = fs::read_to_string(hook_dir.join("stop/process.ts")).unwrap();
    assert!(process_source.contains("spawn(command, args"));
    assert!(process_source.contains("shell: false"));
    assert!(!process_source.contains("spawnSync"));
    let eslint_source = fs::read_to_string(hook_dir.join("stop/runner/eslint.ts")).unwrap();
    assert!(eslint_source.contains("isInAIHook: 'true'"));
    for unexpected in [
        "Related Vitest suites and coverage passed.",
        "Related Vitest suites passed; no AI-edited files matched project coverage rules.",
    ] {
        assert!(!stop_source.contains(unexpected), "unexpected {unexpected}");
    }

    let second = jt()
        .args(["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"])
        .current_dir(&nested)
        .output()
        .unwrap();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("already configured"));
    assert_eq!(fs::read(path).unwrap(), first_bytes);
}

#[test]
fn ai_hook_check_selection_detaches_owned_runner_and_preserves_custom_runner() {
    let project = tempdir().unwrap();
    init_git(project.path());
    write_ai_hook_package(project.path());
    let install = jt()
        .args(["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();
    assert!(install.status.success());

    let runners = project.path().join(".codex/hooks/jt-ai-hook/stop/runner");
    fs::write(
        runners.join("custom.ts"),
        "export async function run() { return { status: 'passed' } }\n",
    )
    .unwrap();
    let detach = jt()
        .args(["ai-hook", "--checks", "vitest", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert!(detach.status.success());
    assert!(runners.join("vitest.ts").is_file());
    assert!(!runners.join("eslint.ts").exists());
    assert!(runners.join("custom.ts").is_file());
    let config = fs::read_to_string(project.path().join(".codex/hooks.json")).unwrap();
    assert_eq!(config.matches("stop-entry.ts codex").count(), 1);
}

#[test]
fn ai_hook_refuses_to_replace_unowned_runner() {
    let project = tempdir().unwrap();
    init_git(project.path());
    write_ai_hook_package(project.path());
    let hook_dir = project.path().join(".codex/hooks/jt-ai-hook/stop/runner");
    fs::create_dir_all(&hook_dir).unwrap();
    let path = hook_dir.join("eslint.ts");
    fs::write(&path, "custom eslint runner\n").unwrap();

    let output = jt()
        .args(["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unowned"));
    assert_eq!(fs::read_to_string(path).unwrap(), "custom eslint runner\n");
    assert!(!project.path().join(".codex/hooks.json").exists());
}

#[test]
fn ai_hook_rejects_invalid_existing_json_without_mutation() {
    let project = tempdir().unwrap();
    init_git(project.path());
    write_ai_hook_package(project.path());
    fs::create_dir(project.path().join(".codex")).unwrap();
    let legacy = project.path().join(".codex/hooks/jt-vitest/stop.ts");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let legacy_original = b"// jt-vitest-ai-hook\n";
    fs::write(&legacy, legacy_original).unwrap();
    let path = project.path().join(".codex/hooks.json");
    let original = b"{not json\n";
    fs::write(&path, original).unwrap();

    let output = jt()
        .args(["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid"));
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(fs::read(legacy).unwrap(), legacy_original);
}

#[cfg(unix)]
#[test]
fn ai_hook_rejects_symlinked_file_without_mutating_target() {
    use std::os::unix::fs::symlink;

    let project = tempdir().unwrap();
    let external = tempdir().unwrap();
    init_git(project.path());
    write_ai_hook_package(project.path());
    fs::create_dir(project.path().join(".codex")).unwrap();
    let target = external.path().join("hooks.json");
    let original = b"{\"external\":true}\n";
    fs::write(&target, original).unwrap();
    let link = project.path().join(".codex/hooks.json");
    symlink(&target, &link).unwrap();

    let output = jt()
        .args(["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlinked file"));
    assert_eq!(fs::read(target).unwrap(), original);
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
}

#[cfg(unix)]
#[test]
fn ai_hook_legacy_cleanup_preserves_symlinked_file_and_target() {
    use std::os::unix::fs::symlink;

    let project = tempdir().unwrap();
    let external = tempdir().unwrap();
    init_git(project.path());
    write_ai_hook_package(project.path());
    let old_dir = project.path().join(".codex/hooks/jt-vitest");
    fs::create_dir_all(&old_dir).unwrap();
    let target = external.path().join("stop.ts");
    let original = b"// jt-vitest-ai-hook\n";
    fs::write(&target, original).unwrap();
    let link = old_dir.join("stop.ts");
    symlink(&target, &link).unwrap();

    let output = jt()
        .args(["ai-hook", "--checks", "vitest", "--agents", "codex"])
        .current_dir(project.path())
        .output()
        .unwrap();

    assert!(output.status.success());
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
