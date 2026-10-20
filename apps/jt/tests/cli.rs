use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use rusqlite::Connection;
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

#[test]
fn help_lists_new_commands_only() {
    let output = jt().arg("--help").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains("jt repo cicd"));
    assert!(stdout.contains("jt node init"));
    assert!(stdout.contains("jt codex init"));
    assert!(!stdout.contains("nlab-api"));
    assert!(stdout.contains("jt cli bootstrap"));
    assert!(stdout.contains("jt ghostty install"));
    assert!(stdout.contains("jt zed-conf"));
    assert!(!stdout.contains("ai-hook"));
    assert!(stdout.contains("jt code unused [PATH]"));
    assert!(stdout.contains("jt unused [PATH]"));
    assert!(stdout.contains("jt call-graph [PATH]"));
    assert!(stdout.contains("jt vitest"));
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
fn codex_init_creates_configuration_without_executing_package_scripts() {
    let project = tempdir().unwrap();
    fs::write(
        project.path().join("package.json"),
        r#"{"scripts":{"dev":"exit 99","postinstall":"exit 99"}}"#,
    )
    .unwrap();
    for message in ["created", "current"] {
        let output = jt()
            .args(["codex", "init"])
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(message));
    }
    let content =
        fs::read_to_string(project.path().join(".codex/environments/environment.toml")).unwrap();
    assert!(content.contains("npm run dev"));
    assert!(!content.contains("Unit tests"));
    let environment = project.path().join(".codex/environments/environment.toml");
    let custom = b"custom configuration, even invalid UTF-8: \xff\n";
    fs::write(&environment, custom).unwrap();
    let preview = jt()
        .args(["codex", "init", "--check"])
        .current_dir(project.path())
        .output()
        .unwrap();
    assert!(preview.status.success());
    assert!(String::from_utf8_lossy(&preview.stdout).contains("candidate:"));
    assert_eq!(fs::read(&environment).unwrap(), custom);
    for option in [None, Some("--update")] {
        fs::write(&environment, custom).unwrap();
        let output = jt()
            .args(["codex", "init"])
            .args(option)
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let (_, backup) = stdout.split_once("; restore from ").unwrap();
        assert_eq!(fs::read(backup.trim()).unwrap(), custom);
        assert_eq!(fs::read_to_string(&environment).unwrap(), content);
    }
    assert!(!project.path().join("node_modules").exists());
}

#[test]
fn unused_help_and_non_project_are_read_only() {
    let help = jt().args(["unused", "--help"]).output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--kind"));
    assert!(help.contains("--mode"));
    assert!(help.contains("--json"));

    let project = tempdir().unwrap();
    let output = jt()
        .args(["unused", project.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error:"));
    assert!(stderr.contains("JavaScript"));
    assert!(stderr.contains("TypeScript"));
    assert!(stderr.contains("Vue"));
    assert!(stderr.contains("project"));
    assert!(project.path().read_dir().unwrap().next().is_none());

    let invalid = jt().args(["unused", "--kind", "class"]).output().unwrap();
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
fn code_unused_help_exposes_same_options_as_legacy_command() {
    let output = jt().args(["code", "unused", "--help"]).output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--kind"));
    assert!(help.contains("--mode"));
    assert!(help.contains("--json"));
}

fn write_unused_fixture(path: &Path) {
    for (relative, contents) in [
        (
            "package.json",
            r#"{"name":"unused-fixture","private":true,"scripts":{"start":"node src/main.ts"},"exports":"./src/index.ts"}"#,
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"target":"ES2022","module":"ESNext","moduleResolution":"Bundler","strict":true},"include":["src"]}"#,
        ),
        (
            "src/main.ts",
            "import './consumer';\nexport function appEntrypoint() {}\n",
        ),
        (
            "src/consumer.ts",
            "import { consumed, importedButUnused } from './defs';\nconsumed();\nimport './side-effect';\nvoid import('./dynamic-target');\n",
        ),
        (
            "src/defs.ts",
            "export function appOnly() {}\nexport function consumed() {}\nexport function importedButUnused() {}\nfunction privateOnly() {}\n",
        ),
        ("src/side-effect.ts", "console.log('side effect');\n"),
        ("src/dynamic-target.ts", "console.log('dynamic target');\n"),
        (
            "src/reexported.ts",
            "function publicHelper() {}\nfunction publicObjectHelper() {}\nexport function reexported() { publicHelper(); }\nexport const objectApi = { run() { publicObjectHelper(); } };\nexport function publicSibling() {}\n",
        ),
        (
            "src/index.ts",
            "export { objectApi, reexported } from './reexported';\n",
        ),
        ("src/types.d.ts", "declare function typeOnly(): void;\n"),
        ("src/example.test.ts", "export function testOnly() {}\n"),
    ] {
        write_file(path, relative, contents);
    }
}

fn write_file(root: &Path, relative: &str, contents: &str) {
    let file = root.join(relative);
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(file, contents).unwrap();
}

fn report_section_names(report: &serde_json::Value, section: &str) -> Vec<String> {
    report[section]
        .as_array()
        .unwrap_or_else(|| panic!("missing JSON section {section}"))
        .iter()
        .filter_map(|item| item.get("name").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn report_finding<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item.get("name").and_then(serde_json::Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("missing finding {name}"))
}

fn golden_entry(item: &serde_json::Value, include_reexports: bool) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "kind": item["kind"],
        "name": item["name"],
        "path": item["path"],
        "line": item["line"],
        "reason": item["reason"],
    });
    if include_reexports {
        entry["reexports"] = item["reexports"].clone();
    }
    entry
}

fn golden_report(report: &serde_json::Value) -> serde_json::Value {
    let entries = |section: &str, include_reexports| {
        report[section]
            .as_array()
            .unwrap_or_else(|| panic!("missing JSON section {section}"))
            .iter()
            .map(|item| golden_entry(item, include_reexports))
            .collect::<Vec<_>>()
    };
    serde_json::json!({
        "findings": entries("findings", true),
        "ignored": entries("ignored", false),
        "unknown": entries("unknown", false),
        "diagnostics": report["diagnostics"]
            .as_array()
            .expect("missing diagnostics")
            .iter()
            .map(|diagnostic| diagnostic["code"].clone())
            .collect::<Vec<_>>(),
        "summary": {
            "scannedFiles": report["summary"]["scannedFiles"],
            "scannedSymbols": report["summary"]["scannedSymbols"],
            "functions": report["summary"]["functions"],
            "variables": report["summary"]["variables"],
            "files": report["summary"]["files"],
            "ignored": report["summary"]["ignored"],
            "unknown": report["summary"]["unknown"],
            "diagnostics": report["summary"]["diagnostics"],
        },
    })
}

fn assert_unused_json_contract(report: &serde_json::Value) {
    assert!(Path::new(report["root"].as_str().expect("root")).is_absolute());
    assert_eq!(report["scope"], ".");
    assert!(report["scanRoots"].is_array());
    assert!(report["exclude"].is_array());
    assert_eq!(report["mode"], "app");
    for finding in report["findings"].as_array().expect("findings") {
        for key in [
            "id",
            "kind",
            "language",
            "name",
            "qualifiedName",
            "path",
            "line",
            "column",
            "reason",
            "reexports",
        ] {
            assert!(finding.get(key).is_some(), "missing {key}: {finding}");
        }
        assert!(finding["line"].as_u64().is_some_and(|line| line >= 1));
        assert!(finding["column"].as_u64().is_some_and(|column| column >= 1));
    }
    for section in ["ignored", "unknown"] {
        for item in report[section]
            .as_array()
            .unwrap_or_else(|| panic!("{section}"))
        {
            for key in ["id", "kind", "name", "path", "line", "column", "reason"] {
                assert!(item.get(key).is_some(), "missing {key}: {item}");
            }
        }
    }
}

#[test]
fn unused_golden_fixture_matches_expected_contract() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unused-golden");
    let expected = serde_json::from_slice::<serde_json::Value>(
        &fs::read(fixture.join("expected.json")).unwrap(),
    )
    .unwrap();
    let isolated_path = tempdir().unwrap();
    let output = jt()
        .args(["unused", fixture.to_str().unwrap(), "--json"])
        .env("PATH", isolated_path.path())
        .env_remove("NODE_PATH")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let actual = serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    assert_unused_json_contract(&actual);
    assert_eq!(golden_report(&actual), expected);
}

#[test]
fn code_unused_matches_legacy_unused_byte_for_byte() {
    let fixture = tempdir().unwrap();
    write_unused_fixture(fixture.path());
    let isolated_path = tempdir().unwrap();
    let project_path = fixture.path().to_str().unwrap();

    let legacy = jt()
        .args([
            "unused",
            project_path,
            "--kind",
            "function,variable,file",
            "--mode",
            "app",
            "--json",
        ])
        .env("PATH", isolated_path.path())
        .env_remove("NODE_PATH")
        .output()
        .unwrap();
    let nested = jt()
        .args([
            "code",
            "unused",
            project_path,
            "--kind",
            "function,variable,file",
            "--mode",
            "app",
            "--json",
        ])
        .env("PATH", isolated_path.path())
        .env_remove("NODE_PATH")
        .output()
        .unwrap();

    assert_eq!(legacy.status.code(), nested.status.code());
    assert_eq!(legacy.stdout, nested.stdout);
    assert_eq!(legacy.stderr, nested.stderr);
    assert!(serde_json::from_slice::<serde_json::Value>(&legacy.stdout).is_ok());
}

fn semantic_dependencies_available(fixture: &Path) -> bool {
    let probe = r#"
const { createRequire } = require('node:module')
const projectRequire = createRequire(`${process.argv[1]}/package.json`)
for (const dependency of ['typescript', 'vue-tsc', '@vue/compiler-sfc']) {
  projectRequire.resolve(dependency)
}
"#;
    Command::new("node")
        .args(["-e", probe, fixture.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn unused_semantic_golden_fixture_matches_expected_contract() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unused-semantic-golden");
    if !semantic_dependencies_available(&fixture) {
        eprintln!(
            "skipped semantic golden fixture: install workspace Node.js dependencies with `pnpm install`"
        );
        return;
    }
    let expected = serde_json::from_slice::<serde_json::Value>(
        &fs::read(fixture.join("expected.json")).unwrap(),
    )
    .unwrap();
    let output = jt()
        .args(["unused", fixture.to_str().unwrap(), "--json"])
        .env_remove("NODE_PATH")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let actual = serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    assert_eq!(golden_report(&actual), expected);
}

#[test]
fn unused_fixture_distinguishes_app_library_and_reexport_usage() {
    let project = tempdir().unwrap();
    write_unused_fixture(project.path());
    let project_path = project.path().to_str().unwrap();

    let app = jt()
        .args(["unused", project_path, "--kind", "function", "--json"])
        .output()
        .unwrap();
    assert!(
        app.status.success(),
        "{}",
        String::from_utf8_lossy(&app.stderr)
    );
    assert!(app.stderr.is_empty());
    let app: serde_json::Value = serde_json::from_slice(&app.stdout).unwrap();
    let app_names = report_section_names(&app, "findings");
    assert!(app_names.contains(&"appOnly".to_owned()), "{app:#}");
    assert!(app_names.contains(&"importedButUnused".to_owned()));
    assert!(app_names.contains(&"privateOnly".to_owned()));
    assert!(app_names.contains(&"reexported".to_owned()));
    assert!(app_names.contains(&"publicSibling".to_owned()));
    assert!(app_names.contains(&"publicHelper".to_owned()));
    assert!(!app_names.contains(&"consumed".to_owned()));
    assert!(app_names.contains(&"appEntrypoint".to_owned()));
    assert!(!app_names.contains(&"testOnly".to_owned()));

    let reexport = report_finding(&app, "reexported");
    assert!(
        reexport
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|reason| reason.contains("reexport"))
    );
    assert!(
        reexport
            .get("reexports")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|locations| !locations.is_empty())
    );

    let library = jt()
        .args([
            "unused",
            project_path,
            "--kind",
            "function",
            "--mode",
            "library",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        library.status.success(),
        "{}",
        String::from_utf8_lossy(&library.stderr)
    );
    assert!(library.stderr.is_empty());
    let library: serde_json::Value = serde_json::from_slice(&library.stdout).unwrap();
    let library_names = report_section_names(&library, "findings");
    assert!(library_names.contains(&"privateOnly".to_owned()));
    assert!(library_names.contains(&"appOnly".to_owned()));
    assert!(!library_names.contains(&"reexported".to_owned()));
    assert!(library_names.contains(&"publicSibling".to_owned()));
    assert!(!library_names.contains(&"publicHelper".to_owned()));
    assert!(!library_names.contains(&"publicObjectHelper".to_owned()));
    assert!(!library_names.contains(&"run".to_owned()));
    let ignored_names = report_section_names(&library, "ignored");
    assert!(!ignored_names.contains(&"appOnly".to_owned()));
    assert!(ignored_names.contains(&"reexported".to_owned()));

    let files = jt()
        .args(["unused", project_path, "--kind", "file", "--json"])
        .output()
        .unwrap();
    assert!(
        files.status.success(),
        "{}",
        String::from_utf8_lossy(&files.stderr)
    );
    let files: serde_json::Value = serde_json::from_slice(&files.stdout).unwrap();
    let file_paths = files["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.get("path").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert!(
        file_paths
            .iter()
            .any(|path| path.ends_with("reexported.ts"))
    );
    let reexported_file = files["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item.get("path")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|path| path.ends_with("reexported.ts"))
        })
        .unwrap();
    assert_eq!(reexported_file["reason"], "reexport-only");
    assert!(
        reexported_file["reexports"]
            .as_array()
            .is_some_and(|locations| !locations.is_empty())
    );
    assert!(
        !file_paths
            .iter()
            .any(|path| path.ends_with("side-effect.ts"))
    );
    assert!(
        !file_paths
            .iter()
            .any(|path| path.ends_with("dynamic-target.ts"))
    );
    assert!(!file_paths.iter().any(|path| path.ends_with("main.ts")));
    assert!(!file_paths.iter().any(|path| path.ends_with("types.d.ts")));
    assert!(
        !file_paths
            .iter()
            .any(|path| path.ends_with("example.test.ts"))
    );
}

#[test]
fn unused_config_limits_candidates_but_keeps_excluded_consumer_evidence() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"unused-config-fixture","private":true}"#,
    );
    write_file(
        project.path(),
        ".nlab/unused.config.json",
        r#"{"version":2,"roots":["src"],"entrypoints":["src/feature/consumer.ts"],"exclude":["src/ignored/**"]}"#,
    );
    write_file(
        project.path(),
        "src/feature/defs.ts",
        "export function usedInScope() {}\nexport function usedOnlyByExcludedConsumer() {}\n",
    );
    write_file(
        project.path(),
        "src/feature/consumer.ts",
        "import { usedInScope } from './defs'; usedInScope();\n",
    );
    write_file(
        project.path(),
        "src/ignored/consumer.ts",
        "import { usedOnlyByExcludedConsumer } from '../feature/defs'; export function generatedConsumer() { usedOnlyByExcludedConsumer(); }\n",
    );
    write_file(
        project.path(),
        "scripts/outside.ts",
        "export function outsideRoot() {}\n",
    );

    let output = jt()
        .args([
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "function",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scanRoots"], serde_json::json!(["src"]));
    assert_eq!(report["exclude"], serde_json::json!(["src/ignored/**"]));
    assert_eq!(report["summary"]["scannedFiles"], 2);
    let findings = report_section_names(&report, "findings");
    assert!(!findings.contains(&"usedOnlyByExcludedConsumer".to_owned()));
    assert!(!findings.contains(&"usedInScope".to_owned()));
    assert!(!findings.contains(&"outsideRoot".to_owned()));

    let outside = jt()
        .args([
            "unused",
            project.path().join("scripts").to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(outside.status.code(), Some(1));
    assert!(outside.stdout.is_empty());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("outside configured roots"));
}

#[test]
fn unused_config_accepts_version_two_entrypoints() {
    let project = tempdir().unwrap();
    write_file(project.path(), "package.json", r#"{"private":true}"#);
    write_file(
        project.path(),
        ".nlab/unused.config.json",
        r#"{"version":2,"roots":["src"],"entrypoints":["src/start.ts"]}"#,
    );
    write_file(
        project.path(),
        "src/start.ts",
        "export function start() {}\n",
    );
    write_file(
        project.path(),
        "src/unused.ts",
        "export function orphan() {}\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "file",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["ignored"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["path"] == "src/start.ts" && item["reason"] == "entrypoint" })
    );
    assert!(
        !report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["path"] == "src/start.ts" })
    );
}

#[test]
fn unused_config_rejects_invalid_contracts_without_writes() {
    for config in [
        r#"{"version":3,"roots":["src"]}"#,
        r#"{"version":1,"roots":["../src"]}"#,
        r#"{"version":1,"roots":["/src"]}"#,
        r#"{"version":1,"roots":["src"],"unknown":true}"#,
        r#"{"version":1,"roots":["src"],"exclude":["!src/keep.ts"]}"#,
        r#"{"version":2,"roots":["src"],"entrypoints":["../outside.ts"]}"#,
        r#"{"version":2,"roots":["src"],"entrypoints":["src/missing.ts"]}"#,
        r#"{"version":1,"roots":["src"],"entrypoints":["src/index.ts"]}"#,
        r#"{"version":1,"roots":["src"],"entrypoints":[]}"#,
        r#"{"version":2,"roots":["src"],"entrypoints":["src/index.html"]}"#,
    ] {
        let project = tempdir().unwrap();
        write_file(project.path(), "package.json", r#"{"private":true}"#);
        write_file(project.path(), "src/index.ts", "export const value = 1;\n");
        write_file(project.path(), "src/index.html", "<main></main>\n");
        write_file(project.path(), ".nlab/unused.config.json", config);
        let before = fs::read(project.path().join(".nlab/unused.config.json")).unwrap();

        let output = jt()
            .args(["unused", project.path().to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{config}");
        assert!(output.stdout.is_empty(), "{config}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unused.config.json"),
            "{config}"
        );
        assert_eq!(
            fs::read(project.path().join(".nlab/unused.config.json")).unwrap(),
            before
        );
    }
}

#[test]
fn unused_build_artifact_entrypoints_do_not_create_phantom_roots() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"built-library","main":"dist/index.js","exports":"./dist/index.js"}"#,
    );
    write_file(
        project.path(),
        "dist/index.js",
        "export const built = true;\n",
    );
    write_file(
        project.path(),
        "src/index.ts",
        "export function publicValue() {}\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--mode",
            "library",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!report_section_names(&report, "findings").contains(&"publicValue".to_owned()));
    assert!(report_section_names(&report, "unknown").contains(&"publicValue".to_owned()));
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["code"] == "entrypoint-coverage-incomplete" })
    );
}

#[test]
fn unused_framework_boundary_uses_framework_unknown_reason() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"nuxt-app","scripts":{"start":"node src/main.ts"},"dependencies":{"nuxt":"latest"}}"#,
    );
    write_file(project.path(), "src/main.ts", "export const boot = true;\n");
    write_file(
        project.path(),
        "src/page.ts",
        "export function conventionPage() {}\n",
    );

    let output = jt()
        .args(["code", "unused", project.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let page = report["unknown"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "conventionPage")
        .expect("framework symbol unknown");
    assert_eq!(page["reason"], "framework-semantic-incomplete");
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["code"] == "framework-semantic-incomplete" })
    );
}

#[test]
fn unused_missing_vue_semantics_downgrades_unreachable_vue_file() {
    let project = tempdir().unwrap();
    let isolated_path = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"vue-without-volar","scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(project.path(), "src/main.ts", "export const boot = true;\n");
    write_file(
        project.path(),
        "src/dead.vue",
        "<script setup lang=\"ts\">\nfunction templateOnly() {}\n</script>\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "file",
            "--json",
        ])
        .env("PATH", isolated_path.path())
        .env_remove("NODE_PATH")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let dead_vue = report["unknown"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "src/dead.vue")
        .expect("Vue file unknown");
    assert_eq!(dead_vue["reason"], "vue-semantic-unavailable");
}

#[test]
fn unused_commonjs_fallback_keeps_require_owner_reachability() {
    let project = tempdir().unwrap();
    let isolated_path = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"commonjs-fallback","scripts":{"start":"node src/main.cjs"}}"#,
    );
    write_file(
        project.path(),
        "src/main.cjs",
        "function dead() { require('./dead-target'); }\nrequire('./live-target');\n",
    );
    write_file(
        project.path(),
        "src/dead-target.cjs",
        "exports.deadTarget = () => {};\n",
    );
    write_file(
        project.path(),
        "src/live-target.cjs",
        "exports.liveTarget = () => {};\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "file",
            "--json",
        ])
        .env("PATH", isolated_path.path())
        .env_remove("NODE_PATH")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|item| item["path"] == "src/dead-target.cjs")
    );
    assert!(
        !findings
            .iter()
            .any(|item| item["path"] == "src/live-target.cjs")
    );
}

#[test]
fn unused_commonjs_export_alone_is_not_app_usage() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"commonjs-export","scripts":{"start":"node src/main.cjs"}}"#,
    );
    write_file(
        project.path(),
        "src/main.cjs",
        "function orphan() {}\nmodule.exports = { orphan };\n",
    );

    for isolate_node in [false, true] {
        let isolated_path = tempdir().unwrap();
        let mut command = jt();
        command.args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "function",
            "--json",
        ]);
        if isolate_node {
            command
                .env("PATH", isolated_path.path())
                .env_remove("NODE_PATH");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            report_section_names(&report, "findings").contains(&"orphan".to_owned()),
            "{report:#}"
        );
    }
}

#[test]
fn unused_commonjs_library_inline_callables_are_public_roots() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"commonjs-library","main":"src/index.cjs"}"#,
    );
    write_file(
        project.path(),
        "src/index.cjs",
        "function methodHelper() {}\nfunction functionHelper() {}\nfunction classHelper() {}\nfunction nestedHelper() {}\nfunction hiddenHelper() {}\nfunction configureDeadExport() {\n  function hidden() { hiddenHelper(); }\n  module.exports.hidden = hidden;\n}\nmodule.exports = {\n  run() { function nestedDead() { nestedHelper(); } methodHelper(); },\n  fn: function () { functionHelper(); },\n  Service: class { field = classHelper(); constructor() { classHelper(); } },\n};\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "function",
            "--mode",
            "library",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report_section_names(&report, "findings");
    for name in ["methodHelper", "functionHelper", "classHelper"] {
        assert!(
            !findings.contains(&name.to_owned()),
            "missing public root for {name}: {report:#}"
        );
    }
    for name in [
        "nestedDead",
        "nestedHelper",
        "configureDeadExport",
        "hidden",
        "hiddenHelper",
    ] {
        assert!(
            findings.contains(&name.to_owned()),
            "missing dead {name}: {report:#}"
        );
    }
}

#[test]
fn unused_reachability_keeps_runtime_owners_and_reports_dead_cycles() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"unused-reachability","private":true,"scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(
        project.path(),
        "src/main.ts",
        "import './reachable'; import './class-runtime'; import './callback';\n",
    );
    write_file(
        project.path(),
        "src/reachable.ts",
        "function live() {}\nlive();\nfunction deadA() { deadB(); }\nfunction deadB() { deadA(); }\nfunction takes(parameter: string) { return parameter; }\nconst _under = 1;\nfunction register() { return 1; }\nconst sideEffect = register();\nfunction nestedHelper() {}\nfunction callbackOnlyHelper() {}\nfunction objectOnlyHelper() {}\nfunction activeObjectHelper() {}\nfunction dormantArrowHelper() {}\nfunction activeArrowHelper() {}\nfunction deadOwner() { const nested = nestedHelper(); class Nested { static { nestedHelper(); } } return nested; }\nconst deadCallback = () => callbackOnlyHelper();\nconst deadRegistry = { handler() { objectOnlyHelper(); } };\nconst activeRegistry = { activeHandler() { activeObjectHelper(); } };\nactiveRegistry[action]();\nconst dormantArrowRegistry = { handler: () => dormantArrowHelper() };\nconst activeArrowRegistry = { handler: () => activeArrowHelper() };\nactiveArrowRegistry[action]();\n",
    );
    write_file(
        project.path(),
        "src/class-runtime.ts",
        "function classHelper() {}\nclass Service { static { classHelper(); } field = classHelper(); constructor() { classHelper(); } }\nnew Service();\n",
    );
    write_file(
        project.path(),
        "src/callback.ts",
        "function callbackHelper() {}\n[1].map(() => callbackHelper());\n",
    );
    write_file(
        project.path(),
        "src/dead-a.ts",
        "import './dead-b'; export function deadFileA() {}\n",
    );
    write_file(
        project.path(),
        "src/dead-b.ts",
        "import './dead-a'; export function deadFileB() {}\n",
    );

    let output = jt()
        .args(["code", "unused", project.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report_section_names(&report, "findings");
    for name in [
        "deadA",
        "deadB",
        "takes",
        "_under",
        "sideEffect",
        "deadFileA",
        "deadFileB",
        "nestedHelper",
        "callbackOnlyHelper",
        "objectOnlyHelper",
        "deadOwner",
        "deadCallback",
        "deadRegistry",
        "dormantArrowHelper",
        "dormantArrowRegistry",
    ] {
        assert!(
            findings.contains(&name.to_owned()),
            "missing {name}: {report:#}"
        );
    }
    for name in [
        "live",
        "register",
        "classHelper",
        "callbackHelper",
        "parameter",
        "field",
        "activeObjectHelper",
        "activeRegistry",
        "activeHandler",
        "activeArrowHelper",
        "activeArrowRegistry",
    ] {
        assert!(
            !findings.contains(&name.to_owned()),
            "unexpected {name}: {report:#}"
        );
    }
    assert_eq!(
        report_finding(&report, "sideEffect")["reason"],
        "unused-binding-side-effectful-initializer"
    );
    assert_eq!(
        report_finding(&report, "deadFileA")["reason"],
        "unreachable-from-entrypoint"
    );
    assert!(report_section_names(&report, "unknown").contains(&"handler".to_owned()));
    let file_findings = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["kind"] == "file")
        .filter_map(|finding| finding["path"].as_str())
        .collect::<Vec<_>>();
    assert!(file_findings.contains(&"src/dead-a.ts"));
    assert!(file_findings.contains(&"src/dead-b.ts"));
}

#[test]
fn unused_instance_field_does_not_activate_imported_helper_before_instantiation() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"field-owner","scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(project.path(), "src/main.ts", "import './service';\n");
    write_file(
        project.path(),
        "src/helpers.ts",
        "export function instanceHelper() {}\nexport function staticHelper() {}\n",
    );
    write_file(
        project.path(),
        "src/service.ts",
        "import { instanceHelper, staticHelper } from './helpers';\nclass Service { field = instanceHelper(); static field = staticHelper(); }\nvoid Service;\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "function",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report_section_names(&report, "findings");
    assert!(
        findings.contains(&"instanceHelper".to_owned()),
        "{report:#}"
    );
    assert!(!findings.contains(&"staticHelper".to_owned()), "{report:#}");
}

#[test]
fn unused_type_consumer_protects_value_and_file_without_protecting_siblings() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unused-type-consumer");
    if !semantic_dependencies_available(&fixture) {
        eprintln!(
            "skipped type consumer fixture: install workspace Node.js dependencies with `pnpm install`"
        );
        return;
    }
    let output = jt()
        .args(["code", "unused", fixture.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report_section_names(&report, "findings");
    assert!(!findings.contains(&"value".to_owned()), "{report:#}");
    assert!(findings.contains(&"unusedRuntime".to_owned()), "{report:#}");
    assert!(
        !report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| { finding["kind"] == "file" && finding["path"] == "src/runtime.ts" }),
        "{report:#}"
    );
}

#[test]
fn unused_coverage_failure_isolated_from_clean_files() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"unused-coverage","private":true,"scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(project.path(), "src/main.ts", "import './broken';\n");
    write_file(
        project.path(),
        "src/broken.ts",
        "export function broken( {\n",
    );
    write_file(
        project.path(),
        "src/clean.ts",
        "export function cleanUnused() {}\n",
    );

    let output = jt()
        .args(["code", "unused", project.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report_section_names(&report, "findings");
    assert!(findings.contains(&"cleanUnused".to_owned()), "{report:#}");
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["path"] == "src/broken.ts" && item["code"] == "oxc" })
    );
}

#[test]
fn unused_dead_dynamic_boundary_does_not_hide_clean_findings() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"dead-dynamic-boundary","scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(
        project.path(),
        "src/main.ts",
        "declare const runtimePath: string;\nfunction deadLoader() { void import(runtimePath); }\nexport function cleanUnused() {}\n",
    );
    write_file(
        project.path(),
        "src/orphan.ts",
        "export function orphanUnused() {}\n",
    );
    write_file(
        project.path(),
        "src/dead-boundary.ts",
        "declare const runtimePath: string;\ndeclare const exportName: string;\nvoid import(runtimePath);\nimport 'virtual:dead';\nmodule.exports[exportName] = {};\n",
    );

    let output = jt()
        .args(["code", "unused", project.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report_section_names(&report, "findings");
    assert!(findings.contains(&"cleanUnused".to_owned()), "{report:#}");
    assert!(findings.contains(&"orphanUnused".to_owned()), "{report:#}");
    assert!(!report_section_names(&report, "unknown").contains(&"cleanUnused".to_owned()));
}

#[test]
fn unused_decorated_method_is_runtime_dispatch_unknown() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"decorated-method","scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(project.path(), "src/main.ts", "import './service';\n");
    write_file(
        project.path(),
        "src/service.ts",
        "declare function register(...args: unknown[]): MethodDecorator;\nclass Service { @register decorated() {} }\nvoid Service;\n",
    );

    let output = jt()
        .args(["code", "unused", project.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let decorated = report["unknown"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "decorated")
        .expect("decorated method unknown");
    assert_eq!(decorated["reason"], "runtime-dispatch-ambiguous");
}

#[test]
fn unused_reexport_only_file_requires_top_level_side_effect_safety() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"unused-reexport-effects","private":true,"scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(project.path(), "src/main.ts", "import './index';\n");
    write_file(
        project.path(),
        "src/index.ts",
        "export { value } from './effect';\nexport { pureValue } from './pure';\nexport type { TypeOnly } from './type-effect';\n",
    );
    write_file(
        project.path(),
        "src/effect.ts",
        "import './effect-dependency';\nfunction boot() {}\nboot();\nconsole.log('effect');\nexport const value = 1;\n",
    );
    write_file(
        project.path(),
        "src/effect-dependency.ts",
        "function dependencyBoot() {}\ndependencyBoot();\n",
    );
    write_file(
        project.path(),
        "src/pure.ts",
        "export const pureValue = 1;\n",
    );
    write_file(
        project.path(),
        "src/type-effect.ts",
        "console.log('type-only effect is erased');\nexport interface TypeOnly {}\n",
    );

    let output = jt()
        .args(["code", "unused", project.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let file_findings = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["kind"] == "file")
        .filter_map(|finding| finding["path"].as_str())
        .collect::<Vec<_>>();
    assert!(!file_findings.contains(&"src/effect.ts"), "{report:#}");
    assert!(file_findings.contains(&"src/pure.ts"), "{report:#}");
    assert!(file_findings.contains(&"src/type-effect.ts"), "{report:#}");
    let finding_names = report_section_names(&report, "findings");
    assert!(!finding_names.contains(&"boot".to_owned()), "{report:#}");
    assert!(
        !finding_names.contains(&"dependencyBoot".to_owned()),
        "{report:#}"
    );
    assert_eq!(
        report_finding(&report, "pureValue")["reason"],
        "reexport-only"
    );
}

#[test]
fn unused_static_dynamic_route_is_reachable() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"dynamic-route","scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(
        project.path(),
        "src/main.ts",
        "import routes from './routes';\nvoid routes;\n",
    );
    write_file(
        project.path(),
        "src/routes.ts",
        "export default [{ component: () => import('./page') }];\n",
    );
    write_file(
        project.path(),
        "src/page.ts",
        "export function pageEntry() {}\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "file",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        !report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["path"] == "src/page.ts"),
        "{report:#}"
    );
}

#[test]
fn unused_anonymous_default_callable_activates_runtime_owner() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"anonymous-default","scripts":{"start":"node src/main.ts"}}"#,
    );
    write_file(
        project.path(),
        "src/main.ts",
        "import run from './function';\nimport Service from './service';\nrun();\nnew Service();\n",
    );
    write_file(
        project.path(),
        "src/function.ts",
        "function functionHelper() {}\nexport default function () { functionHelper(); }\n",
    );
    write_file(
        project.path(),
        "src/service.ts",
        "function classHelper() {}\nexport default class { field = classHelper(); constructor() { classHelper(); } }\n",
    );

    let output = jt()
        .args([
            "code",
            "unused",
            project.path().to_str().unwrap(),
            "--kind",
            "function",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let findings = report_section_names(&report, "findings");
    assert!(
        !findings.contains(&"functionHelper".to_owned()),
        "{report:#}"
    );
    assert!(!findings.contains(&"classHelper".to_owned()), "{report:#}");
}

#[test]
fn call_graph_writes_deterministic_owned_html() {
    let project = tempdir().unwrap();
    write_file(
        project.path(),
        "package.json",
        r#"{"name":"call-graph-fixture","private":true}"#,
    );
    write_file(
        project.path(),
        "src/calls.ts",
        "function callee() {}\nfunction caller() { callee(); }\ncaller();\n",
    );
    let project_path = project.path().to_str().unwrap();

    let first = jt()
        .args([
            "call-graph",
            project_path,
            "--output",
            "graph.html",
            "--focus",
            "caller",
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let html_path = project.path().join("graph.html");
    let first_html = fs::read_to_string(&html_path).unwrap();
    assert!(first_html.contains("generated by jt call-graph"));
    assert!(!first_html.contains("__JT_GRAPH_DATA__"));
    assert!(first_html.contains("\"kind\":\"calls\""));
    assert!(first_html.contains("callee"));
    assert!(!first_html.contains("entrypoint-coverage-incomplete"));
    let database_path = project.path().join(".nlab/unused-graph.db");
    let database = Connection::open(&database_path).unwrap();
    assert_eq!(
        database
            .query_row(
                "SELECT value FROM project_metadata WHERE key = 'generator'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "jt call-graph"
    );
    assert_eq!(
        database
            .query_row(
                "SELECT COUNT(*)
                 FROM edges edge
                 JOIN nodes source ON source.id = edge.source
                 JOIN nodes target ON target.id = edge.target
                 WHERE edge.kind = 'calls'
                   AND source.name = 'caller'
                   AND target.name = 'callee'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        database
            .query_row(
                "SELECT edge.col
                 FROM edges edge
                 JOIN nodes source ON source.id = edge.source
                 JOIN nodes target ON target.id = edge.target
                 WHERE edge.kind = 'calls'
                   AND source.name = 'caller'
                   AND target.name = 'callee'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        20
    );
    drop(database);

    let second = jt()
        .args([
            "call-graph",
            project_path,
            "--output",
            "graph.html",
            "--focus",
            "caller",
        ])
        .output()
        .unwrap();
    assert!(second.status.success());
    assert_eq!(fs::read_to_string(&html_path).unwrap(), first_html);

    write_file(project.path(), "manual.html", "<p>owned by user</p>\n");
    let refused = jt()
        .args(["call-graph", project_path, "--output", "manual.html"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("unowned HTML"));
    assert_eq!(
        fs::read_to_string(project.path().join("manual.html")).unwrap(),
        "<p>owned by user</p>\n"
    );

    write_file(project.path(), "manual.db", "owned by user\n");
    let refused = jt()
        .args([
            "call-graph",
            project_path,
            "--output",
            "graph.html",
            "--database",
            "manual.db",
            "--force",
        ])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force-database"));
    assert_eq!(
        fs::read_to_string(project.path().join("manual.db")).unwrap(),
        "owned by user\n"
    );

    let replaced = jt()
        .args([
            "call-graph",
            project_path,
            "--output",
            "graph.html",
            "--database",
            "manual.db",
            "--force-database",
        ])
        .output()
        .unwrap();
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(
        Connection::open(project.path().join("manual.db"))
            .unwrap()
            .query_row(
                "SELECT value FROM project_metadata WHERE key = 'generator'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "jt call-graph"
    );
}

#[cfg(unix)]
#[test]
fn call_graph_rejects_symlinked_default_output_directory() {
    use std::os::unix::fs::symlink;

    let project = tempdir().unwrap();
    let outside = tempdir().unwrap();
    write_file(project.path(), "package.json", r#"{"private":true}"#);
    write_file(project.path(), "src/index.ts", "function value() {}\n");
    symlink(outside.path(), project.path().join(".nlab")).unwrap();

    let output = jt()
        .args(["call-graph", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("through symlink"));
    assert!(!outside.path().join("call-graph.html").exists());
    assert!(!outside.path().join("unused-graph.db").exists());
}

#[test]
fn removed_nlab_api_command_preserves_project_files() {
    let project = tempdir().unwrap();
    let files = [
        (".nlab/nlab-api.config.json", r#"{"version":1}"#),
        (
            ".nlab/nlab-api.local.json",
            r#"{"version":1,"runner":"jt"}"#,
        ),
        ("src/api/generated.ts", "export const generated = true;\n"),
    ];
    for (relative, contents) in files {
        write_file(project.path(), relative, contents);
    }

    for arguments in [["nlab-api", "--help"], ["nlab-api", "generate"]] {
        let output = jt()
            .args(arguments)
            .current_dir(project.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand 'nlab-api'")
        );
    }

    for (relative, contents) in files {
        assert_eq!(
            fs::read_to_string(project.path().join(relative)).unwrap(),
            contents
        );
    }
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
fn removed_ai_hook_command_preserves_project_files() {
    let project = tempdir().unwrap();
    let files = [
        (".codex/hooks.json", "{\"hooks\":{\"Stop\":[]}}\n"),
        (
            ".codex/hooks/jt-ai-hook/stop-entry.ts",
            "// jt-ai-hook\nconsole.log('keep');\n",
        ),
        ("hook-log.jsonl", "{\"status\":\"passed\"}\n"),
    ];
    for (relative, contents) in files {
        write_file(project.path(), relative, contents);
    }

    for arguments in [
        &["ai-hook"][..],
        &["ai-hook", "--help"][..],
        &["ai-hook", "--checks", "vitest,eslint", "--agents", "codex"][..],
    ] {
        let output = jt()
            .args(arguments)
            .env("JT_AI_HOOK_LOG", project.path().join("hook-log.jsonl"))
            .current_dir(project.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand 'ai-hook'")
        );
    }

    for (relative, contents) in files {
        assert_eq!(
            fs::read_to_string(project.path().join(relative)).unwrap(),
            contents
        );
    }
}

#[test]
fn vitest_is_placeholder() {
    let output = jt().arg("vitest").output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not implemented yet"));
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
