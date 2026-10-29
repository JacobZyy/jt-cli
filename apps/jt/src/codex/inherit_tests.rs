use super::*;
use std::path::PathBuf;

fn repository() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("primary");
    let target = directory.path().join("linked");
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "--quiet"]).unwrap();
    git(
        &source,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    )
    .unwrap();
    git(
        &source,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "worktree",
            "add",
            "--quiet",
            "--detach",
            target.to_str().unwrap(),
        ],
    )
    .unwrap();
    (directory, source, target)
}

fn write(root: &Path, name: &str, value: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

#[test]
fn manifest_migrates_broad_directories_preserves_custom_patterns_and_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let original =
        "# Personal\n.nlab\n/.codex/**\n.claude/\nprivate/settings.ini\n!.env.test.local\n";
    fs::write(directory.path().join(MANIFEST), original).unwrap();
    update_manifest(directory.path(), &["lefthook-local.yml"]).unwrap();
    let expected = fs::read_to_string(directory.path().join(MANIFEST)).unwrap();
    assert!(expected.contains("private/settings.ini\n!.env.test.local\n"));
    assert!(
        !expected
            .lines()
            .any(|line| line == ".nlab" || line == "/.codex/**" || line == ".claude/")
    );
    assert!(expected.contains("lefthook-local.yml"));
    update_manifest(directory.path(), &["lefthook-local.yml"]).unwrap();
    assert_eq!(
        fs::read_to_string(directory.path().join(MANIFEST)).unwrap(),
        expected
    );
}

#[test]
fn explicit_inheritance_copies_missing_configs_and_preserves_independent_values() {
    let (_directory, source, target) = repository();
    update_manifest(&source, &[]).unwrap();
    write(&source, ".env.local", "source");
    write(&source, ".env.dev.local", "development");
    write(&source, ".codex/config.toml", "model = 'test'\n");
    write(&source, ".claude/settings.local.json", "{}");
    write(&source, ".nlab/.chatgpt-hooks.lock", "lock");
    write(&source, ".nlab/reports/a.local.json", "report");
    write(&source, ".codegraph/codegraph.db", "index");
    write(&source, ".codex/logs/a.log", "log");
    write(&target, ".env.local", "target");
    assert_eq!(copy(&target, None).unwrap(), 3);
    assert_eq!(
        fs::read_to_string(target.join(".env.local")).unwrap(),
        "target"
    );
    assert!(!target.join(".nlab/.chatgpt-hooks.lock").exists());
    assert!(!target.join(".nlab/reports/a.local.json").exists());
    assert!(!target.join(".codegraph").exists());
    assert!(!target.join(".codex/logs").exists());
    write(&source, ".env.dev.local", "changed");
    assert_eq!(copy(&target, None).unwrap(), 0);
    assert_eq!(
        fs::read_to_string(target.join(".env.dev.local")).unwrap(),
        "development"
    );
}

#[test]
fn target_manifest_and_custom_exclusions_take_priority() {
    let (_directory, source, target) = repository();
    write(&source, ".env.local", "local");
    write(&source, ".env.test.local", "test");
    write(&source, "custom/private.ini", "custom");
    write(&target, MANIFEST, "!.env.test.local\ncustom/private.ini\n");
    update_manifest(&target, &[]).unwrap();
    assert_eq!(copy(&target, None).unwrap(), 2);
    assert!(!target.join(".env.test.local").exists());
    assert!(target.join("custom/private.ini").exists());
}

#[test]
fn old_broad_manifest_does_not_copy_locks_logs_or_indexes() {
    let (_directory, source, target) = repository();
    write(&target, MANIFEST, ".nlab\n");
    write(&source, ".nlab/tool.local.json", "{}");
    write(&source, ".nlab/.chatgpt-hooks.lock", "lock");
    write(&source, ".nlab/tool.log", "log");
    write(&source, ".nlab/unused-graph.db", "index");
    write(&source, ".nlab/reports/report.json", "report");
    assert_eq!(copy(&target, None).unwrap(), 1);
    assert!(target.join(".nlab/tool.local.json").exists());
    assert!(!target.join(".nlab/.chatgpt-hooks.lock").exists());
    assert!(!target.join(".nlab/tool.log").exists());
    assert!(!target.join(".nlab/unused-graph.db").exists());
    assert!(!target.join(".nlab/reports").exists());
}

#[test]
fn valid_legacy_nlab_config_prevents_shadowing_but_invalid_legacy_does_not() {
    for (legacy, preserved) in [
        (r#"{"version":1,"runner":"jt"}"#, true),
        (r#"{"version":1,"runner":"nlab-api"}"#, true),
        (r#"{"version":2,"runner":"nlab"}"#, false),
        (r#"{"version":1,"runner":"nlab","unexpected":1}"#, false),
        ("broken JSON", false),
    ] {
        let (_directory, source, target) = repository();
        update_manifest(&source, &[]).unwrap();
        write(
            &source,
            ".nlab/nlab-api.local.json",
            r#"{"version":1,"backend":{}}"#,
        );
        write(
            &source,
            ".nlab/cli.local.json",
            r#"{"version":1,"runner":"nlab"}"#,
        );
        write(&target, ".nlab/cli.local.json", legacy);
        assert_eq!(copy(&target, None).unwrap(), usize::from(!preserved));
        assert_eq!(
            target.join(".nlab/nlab-api.local.json").exists(),
            !preserved
        );
        assert_eq!(
            fs::read_to_string(target.join(".nlab/cli.local.json")).unwrap(),
            legacy
        );
    }
}

#[test]
fn target_lefthook_local_alias_prevents_shadowing() {
    let (_directory, source, target) = repository();
    update_manifest(
        &source,
        &["lefthook-local.yml", ".config/lefthook-local.yml"],
    )
    .unwrap();
    write(
        &source,
        "lefthook-local.yml",
        "pre-commit:\n  parallel: true\n",
    );
    write(
        &target,
        "lefthook-local.json",
        "{\"pre-commit\":{\"parallel\":false}}",
    );
    write(
        &source,
        ".config/lefthook-local.yml",
        "pre-commit:\n  parallel: true\n",
    );
    write(
        &target,
        ".config/lefthook-local.toml",
        "[pre-commit]\nparallel = false\n",
    );
    assert_eq!(copy(&target, None).unwrap(), 0);
    assert!(!target.join("lefthook-local.yml").exists());
    assert!(!target.join(".config/lefthook-local.yml").exists());
}

#[test]
fn lefthook_local_aliases_preserve_target_across_dotted_and_config_paths() {
    for (source_name, target_name) in [
        ("lefthook-local.yml", ".lefthook-local.yml"),
        (".lefthook-local.json", "lefthook-local.yml"),
        ("lefthook-local.yml", ".config/lefthook-local.toml"),
        (".config/lefthook-local.yml", ".lefthook-local.json"),
    ] {
        let (_directory, source, target) = repository();
        update_manifest(&source, &[source_name]).unwrap();
        write(&source, source_name, "SOURCE");
        write(&target, target_name, "TARGET");
        assert_eq!(
            copy(&target, None).unwrap(),
            0,
            "{source_name} shadows {target_name}"
        );
        assert!(!target.join(source_name).exists());
        assert_eq!(
            fs::read_to_string(target.join(target_name)).unwrap(),
            "TARGET"
        );
    }
}

#[test]
fn conflicting_primary_lefthook_local_paths_are_reported() {
    for second_name in [".lefthook-local.yml", ".config/lefthook-local.json"] {
        let (_directory, source, target) = repository();
        update_manifest(&source, &["lefthook-local.yml", second_name]).unwrap();
        write(&source, "lefthook-local.yml", "SOURCE");
        write(&source, second_name, "OTHER");
        assert!(
            copy(&target, None)
                .unwrap_err()
                .contains("multiple Lefthook local override")
        );
        assert!(!target.join("lefthook-local.yml").exists());
        assert!(!target.join(second_name).exists());
    }
}

#[test]
fn conflicting_primary_lefthook_local_formats_are_reported() {
    let (_directory, source, target) = repository();
    update_manifest(&source, &["lefthook-local.yml", "lefthook-local.json"]).unwrap();
    write(
        &source,
        "lefthook-local.yml",
        "pre-commit:\n  parallel: true\n",
    );
    write(
        &source,
        "lefthook-local.json",
        "{\"pre-commit\":{\"parallel\":false}}",
    );
    assert!(
        copy(&target, None)
            .unwrap_err()
            .contains("multiple Lefthook local override formats")
    );
    assert!(!target.join("lefthook-local.yml").exists());
    assert!(!target.join("lefthook-local.json").exists());
}

#[test]
fn checkout_requires_creation_event_and_linked_worktree() {
    let (_directory, source, target) = repository();
    update_manifest(&source, &[]).unwrap();
    write(&source, ".env.local", "local");
    let mut args = vec!["a".repeat(40), "b".repeat(40), "1".to_owned()];
    assert_eq!(copy(&target, Some(&args)).unwrap(), 0);
    args[0] = "0".repeat(40);
    args[2] = "0".to_owned();
    assert_eq!(copy(&target, Some(&args)).unwrap(), 0);
    args[2] = "1".to_owned();
    assert_eq!(copy(&source, Some(&args)).unwrap(), 0);
    assert_eq!(copy(&target, Some(&args)).unwrap(), 1);
}

#[test]
fn plain_project_inheritance_is_noop() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(copy(directory.path(), None).unwrap(), 0);
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_copy_paths_and_manifest_escapes() {
    use std::os::unix::fs::symlink;
    let (directory, source, target) = repository();
    update_manifest(&source, &[]).unwrap();
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).unwrap();
    write(&source, ".codex/config.toml", "model = 'test'");
    symlink(&outside, target.join(".codex")).unwrap();
    assert!(
        copy(&target, None)
            .unwrap_err()
            .contains("refusing non-directory")
    );
    assert!(!outside.join("config.toml").exists());
    fs::remove_file(target.join(".codex")).unwrap();
    fs::remove_file(source.join(".codex/config.toml")).unwrap();
    symlink(&outside, source.join(".env.local")).unwrap();
    assert!(
        copy(&target, None)
            .unwrap_err()
            .contains("refusing non-file local configuration")
    );
    fs::remove_file(source.join(".env.local")).unwrap();
    write(&target, MANIFEST, "../outside/*\n");
    assert!(
        copy(&target, None)
            .unwrap_err()
            .contains("pattern escapes project")
    );
}
