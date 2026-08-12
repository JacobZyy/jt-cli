use std::{
    collections::BTreeSet,
    env,
    ffi::OsString,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value};
use tempfile::tempdir;

use crate::node::fs::{atomic_write, read_optional};

const OWNED_COMMAND: &str = "jt __vitest-hook";
const MAX_HOOK_INPUT: usize = 64 * 1024;
const MAX_REPORT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;
const MAX_REASON_CHARS: usize = 8_000;
const VITEST_TIMEOUT: Duration = Duration::from_secs(120);

pub fn install(target: &OsString) -> u8 {
    match target.to_string_lossy().as_ref() {
        "--codex" => match install_codex() {
            Ok(status) => {
                println!("{}; review and trust repository hook with /hooks", status);
                0
            }
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        },
        "--claude" => {
            eprintln!("error: Claude support is not implemented");
            1
        }
        _ => {
            eprintln!("error: usage: jt vitest ai-hook <--codex|--claude>");
            2
        }
    }
}

fn install_codex() -> Result<String, String> {
    let cwd =
        env::current_dir().map_err(|error| format!("cannot read current directory: {error}"))?;
    let root = git_root(&cwd)?;
    let package = root.join("package.json");
    let package_bytes = fs::read(&package)
        .map_err(|error| format!("cannot read {}: {error}", package.display()))?;
    let package_json: Value = serde_json::from_slice(&package_bytes)
        .map_err(|error| format!("invalid {}: {error}", package.display()))?;
    if !has_direct_vitest(&package_json) {
        return Err(
            "root package.json does not declare vitest; add it before installing this hook"
                .to_owned(),
        );
    }

    let path = root.join(".codex/hooks.json");
    reject_symlink_path(&root, &path)?;
    let current = read_optional(&path).map_err(|error| error.to_string())?;
    let mut hooks = match current.as_deref() {
        Some(bytes) => serde_json::from_slice::<Value>(bytes)
            .map_err(|error| format!("invalid {}: {error}", path.display()))?,
        None => Value::Object(Map::new()),
    };
    merge_hooks(&mut hooks)?;
    let mut next = serde_json::to_vec_pretty(&hooks)
        .map_err(|error| format!("serialize {}: {error}", path.display()))?;
    next.push(b'\n');
    if current.as_deref() != Some(next.as_slice()) {
        atomic_write(&root, &path, current.as_deref(), &next).map_err(|error| error.to_string())?;
        Ok(format!(
            "{} {}",
            if current.is_some() {
                "updated"
            } else {
                "created"
            },
            path.display()
        ))
    } else {
        Ok(format!("already configured {}", path.display()))
    }
}

fn git_root(cwd: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("cannot run git to resolve repository root: {error}"))?;
    if !output.status.success() {
        return Err("current directory is not inside a Git repository".to_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let root = PathBuf::from(text.trim());
    if root.as_os_str().is_empty() {
        return Err("Git returned an empty repository root".to_owned());
    }
    root.canonicalize()
        .map_err(|error| format!("cannot resolve Git repository root: {error}"))
}

fn has_direct_vitest(package: &Value) -> bool {
    [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ]
    .iter()
    .any(|field| {
        package
            .get(*field)
            .and_then(Value::as_object)
            .is_some_and(|dependencies| dependencies.contains_key("vitest"))
    })
}

fn reject_symlink_path(root: &Path, path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "refuse to replace symlinked file: {}",
            path.display()
        ));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("hook path is outside Git root: {}", path.display()))?;
    let mut cursor = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        cursor.push(component.as_os_str());
        if fs::symlink_metadata(&cursor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(format!(
                "refuse to write through symlinked parent: {}",
                cursor.display()
            ));
        }
    }
    Ok(())
}

fn merge_hooks(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "existing .codex/hooks.json must contain a JSON object".to_owned())?;
    let hooks = match object.get_mut("hooks") {
        Some(value) => value
            .as_object_mut()
            .ok_or_else(|| "existing .codex/hooks.json hooks must be an object".to_owned())?,
        None => {
            object.insert("hooks".to_owned(), Value::Object(Map::new()));
            object
                .get_mut("hooks")
                .and_then(Value::as_object_mut)
                .expect("inserted hooks object")
        }
    };
    let stop = hooks
        .entry("Stop".to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    let stop_groups = stop
        .as_array_mut()
        .ok_or_else(|| "existing .codex/hooks.json hooks.Stop must be an array".to_owned())?;
    let owned = owned_handler();
    let mut found = false;
    let mut retained = Vec::with_capacity(stop_groups.len() + 1);
    for mut group in stop_groups.drain(..) {
        let Some(group_object) = group.as_object_mut() else {
            return Err("existing .codex/hooks.json hooks.Stop entries must be objects".to_owned());
        };
        let Some(handlers) = group_object.get_mut("hooks").and_then(Value::as_array_mut) else {
            retained.push(group);
            continue;
        };
        let mut next_handlers = Vec::with_capacity(handlers.len());
        for handler in handlers.drain(..) {
            let is_owned = handler
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command == OWNED_COMMAND);
            if is_owned {
                if !found {
                    next_handlers.push(owned.clone());
                    found = true;
                }
            } else {
                next_handlers.push(handler);
            }
        }
        *handlers = next_handlers;
        if handlers.is_empty() && group_object.len() == 1 && group_object.contains_key("hooks") {
            continue;
        }
        retained.push(group);
    }
    if !found {
        retained.push(Value::Object(Map::from_iter([(
            "hooks".to_owned(),
            Value::Array(vec![owned]),
        )])));
    }
    *stop_groups = retained;
    Ok(())
}

fn owned_handler() -> Value {
    Value::Object(Map::from_iter([
        ("type".to_owned(), Value::String("command".to_owned())),
        (
            "command".to_owned(),
            Value::String(OWNED_COMMAND.to_owned()),
        ),
        ("timeout".to_owned(), Value::Number(150.into())),
        (
            "statusMessage".to_owned(),
            Value::String("Running Vitest".to_owned()),
        ),
    ]))
}

pub fn run_hook() -> u8 {
    let response = run_hook_inner();
    println!(
        "{}",
        serde_json::to_string(&response).unwrap_or_else(|_| {
            "{\"continue\":true,\"systemMessage\":\"Vitest hook could not format its result\"}"
                .to_owned()
        })
    );
    0
}

fn run_hook_inner() -> Value {
    let input = match read_bounded_stdin(MAX_HOOK_INPUT) {
        Ok(input) => input,
        Err(error) => return warning_response(format!("Vitest hook input unavailable: {error}")),
    };
    let input: Value = match serde_json::from_slice(&input) {
        Ok(input) => input,
        Err(error) => return warning_response(format!("Vitest hook input is invalid: {error}")),
    };
    let object = match input.as_object() {
        Some(object) => object,
        None => return warning_response("Vitest hook input must be a JSON object".to_owned()),
    };
    let stop_hook_active = object
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match run_hook_checked(object, stop_hook_active) {
        Ok(response) => response,
        Err(error) => failure_response(
            stop_hook_active,
            format!("Vitest hook setup failed: {error}"),
        ),
    }
}

fn run_hook_checked(object: &Map<String, Value>, stop_hook_active: bool) -> Result<Value, String> {
    let cwd = object
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or(
            env::current_dir()
                .map_err(|error| format!("cannot read current directory: {error}"))?,
        );
    let root = git_root(&cwd)?;
    let vitest = root.join("node_modules/.bin/vitest");
    if !vitest.is_file() {
        return Ok(failure_response(
            stop_hook_active,
            format!(
                "Vitest executable missing at {}; install repository dependencies, then run `vitest run`",
                vitest.display()
            ),
        ));
    }

    let temp = tempdir().map_err(|error| format!("create temporary report directory: {error}"))?;
    let report_path = temp.path().join("report.json");
    let stdout_path = temp.path().join("stdout.txt");
    let stderr_path = temp.path().join("stderr.txt");
    let status = run_vitest(&vitest, &root, &report_path, &stdout_path, &stderr_path)?;
    // Process status is authoritative. A zero status always lets Codex stop,
    // even when a reporter emits stale or contradictory metadata.
    if status == 0 {
        return Ok(serde_json::json!({"continue": true}));
    }
    let report_bytes = match read_bounded_file(&report_path, MAX_REPORT_BYTES) {
        Ok(bytes) => bytes,
        Err(_error) if !report_path.exists() => Vec::new(),
        Err(error) => return Err(error),
    };
    let stdout = read_bounded_file(&stdout_path, MAX_CAPTURE_BYTES)?;
    let stderr = read_bounded_file(&stderr_path, MAX_CAPTURE_BYTES)?;
    let report = if report_bytes.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice::<Value>(&report_bytes)
                .map_err(|error| format!("invalid Vitest JSON report: {error}"))?,
        )
    };
    let summary = normalize_report(report.as_ref(), &stdout, &stderr, &root, status);
    let reason = if summary.is_empty() {
        "Vitest failed without a recognized diagnostic; run `vitest run` for full local detail"
            .to_owned()
    } else {
        summary
    };
    Ok(failure_response(stop_hook_active, reason))
}

fn run_vitest(
    vitest: &Path,
    root: &Path,
    report_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<i32, String> {
    let stdout = File::create(stdout_path)
        .map_err(|error| format!("create Vitest stdout capture: {error}"))?;
    let stderr = File::create(stderr_path)
        .map_err(|error| format!("create Vitest stderr capture: {error}"))?;
    let mut child = Command::new(vitest);
    child
        .current_dir(root)
        .args([
            "run",
            "--reporter=json",
            "--reporter=tap-flat",
            "--reporter=default",
            "--silent",
            "--no-color",
        ])
        .arg(format!("--outputFile.json={}", report_path.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = child
        .spawn()
        .map_err(|error| format!("cannot start repository Vitest executable: {error}"))?;
    let started = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("cannot inspect Vitest process: {error}"))?
        {
            Some(status) => break status.code().unwrap_or(1),
            None if started.elapsed() >= VITEST_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break 124;
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };
    Ok(status)
}

fn read_bounded_stdin(limit: usize) -> Result<Vec<u8>, String> {
    read_bounded(&mut io::stdin().lock(), limit, "hook input")
}

fn read_bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            format!(
                "{} was not produced",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        } else {
            format!("read {}: {error}", path.display())
        }
    })?;
    read_bounded(&mut file, limit, &path.display().to_string())
}

fn read_bounded(reader: &mut impl Read, limit: usize, label: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {label}: {error}"))?;
    if bytes.len() > limit {
        return Err(format!("{label} exceeds {} bytes", limit));
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FailureKind {
    Assertion,
    Suite,
    Snapshot,
    Timeout,
    Unhandled,
    Discovery,
    Coverage,
    Runtime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Failure {
    kind: FailureKind,
    file: Option<String>,
    test: Option<String>,
    location: Option<String>,
    message: String,
    actual: Option<String>,
    expected: Option<String>,
    affected: usize,
}

fn normalize_report(
    report: Option<&Value>,
    stdout: &[u8],
    stderr: &[u8],
    root: &Path,
    status: i32,
) -> String {
    if status == 0 {
        return String::new();
    }
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let terminal = format!("{stdout}\n{stderr}");
    let mut failures = Vec::new();
    if let Some(report) = report {
        parse_json_report(
            report,
            root,
            has_coverage_threshold(&terminal),
            &mut failures,
        );
    }
    parse_tap(&terminal, root, &mut failures);
    parse_terminal_fallback(&terminal, root, &mut failures);
    merge_failures(&mut failures);
    if status == 0 && failures.is_empty() {
        return String::new();
    }
    if status == 124 {
        failures.push(Failure {
            kind: FailureKind::Runtime,
            file: None,
            test: None,
            location: None,
            message: "Vitest timed out after 120 seconds".to_owned(),
            actual: None,
            expected: None,
            affected: 1,
        });
    } else if status != 0 && failures.is_empty() {
        failures.push(Failure {
            kind: FailureKind::Runtime,
            file: None,
            test: None,
            location: None,
            message: "Vitest exited with a non-zero status; run `vitest run` for full local detail"
                .to_owned(),
            actual: None,
            expected: None,
            affected: 1,
        });
    }
    render_failures(&failures)
}

fn parse_json_report(
    report: &Value,
    root: &Path,
    include_coverage: bool,
    failures: &mut Vec<Failure>,
) {
    if let Some(files) = report.get("testResults").and_then(Value::as_array) {
        for file in files {
            let file_path = file
                .get("name")
                .and_then(Value::as_str)
                .map(|name| normalize_path(name, root));
            let nested = file
                .get("assertionResults")
                .or_else(|| file.get("testResults"))
                .and_then(Value::as_array);
            if let Some(assertions) = nested {
                for assertion in assertions {
                    let status = assertion
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !matches!(status, "failed" | "failing") {
                        continue;
                    }
                    let test = assertion
                        .get("title")
                        .or_else(|| assertion.get("fullName"))
                        .or_else(|| assertion.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let messages = assertion
                        .get("failureMessages")
                        .and_then(Value::as_array)
                        .map(|messages| {
                            messages
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    let lower = messages.to_ascii_lowercase();
                    let kind = if lower.contains("snapshot") {
                        FailureKind::Snapshot
                    } else if lower.contains("timed out") || lower.contains("timeout") {
                        FailureKind::Timeout
                    } else if is_hook_message(&lower) {
                        FailureKind::Suite
                    } else {
                        FailureKind::Assertion
                    };
                    failures.push(Failure {
                        kind,
                        file: file_path.clone(),
                        test,
                        location: extract_location(&messages, root),
                        message: clean_message(&messages)
                            .unwrap_or_else(|| "test assertion failed".to_owned()),
                        actual: None,
                        expected: None,
                        affected: 1,
                    });
                }
            }
            let file_message = file
                .get("message")
                .or_else(|| file.get("failureMessage"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let file_status = file
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !file_message.trim().is_empty()
                && (file_status == "failed" || nested.is_none_or(|items| items.is_empty()))
            {
                let lower = file_message.to_ascii_lowercase();
                let kind = if lower.contains("no test") || lower.contains("no matching") {
                    FailureKind::Discovery
                } else if lower.contains("snapshot") {
                    FailureKind::Snapshot
                } else {
                    FailureKind::Suite
                };
                failures.push(Failure {
                    kind,
                    file: file_path.clone(),
                    test: None,
                    location: extract_location(file_message, root),
                    message: clean_message(file_message)
                        .unwrap_or_else(|| "test suite failed".to_owned()),
                    actual: None,
                    expected: None,
                    affected: 1,
                });
            }
        }
    }

    if let Some(snapshot) = report.get("snapshot") {
        let unmatched = snapshot
            .get("unmatched")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let files_unmatched = snapshot
            .get("filesUnmatched")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if (unmatched > 0 || files_unmatched > 0)
            && !failures
                .iter()
                .any(|failure| failure.kind == FailureKind::Snapshot)
        {
            failures.push(Failure {
                kind: FailureKind::Snapshot,
                file: None,
                test: None,
                location: None,
                message: format!("{unmatched} unmatched snapshot(s) in {files_unmatched} file(s)"),
                actual: None,
                expected: None,
                affected: 1,
            });
        }
    }
    if include_coverage {
        if let Some(coverage) = report.get("coverageMap") {
            parse_coverage_map(coverage, root, failures);
        }
    }
    let total_files = report
        .get("numTotalTestSuites")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let success = report
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !success && total_files == 0 && failures.is_empty() {
        failures.push(Failure {
            kind: FailureKind::Discovery,
            file: None,
            test: None,
            location: None,
            message: "no matching test files found".to_owned(),
            actual: None,
            expected: None,
            affected: 1,
        });
    }
}

fn is_hook_message(message: &str) -> bool {
    message.contains("beforeeach")
        || message.contains("aftereach")
        || message.contains("before all")
        || message.contains("after all")
        || message.contains("setup")
        || message.contains("teardown")
}

fn parse_coverage_map(coverage: &Value, root: &Path, failures: &mut Vec<Failure>) {
    let Some(files) = coverage.as_object() else {
        return;
    };
    for (name, data) in files {
        let Some(statement_map) = data.get("statementMap").and_then(Value::as_object) else {
            continue;
        };
        let Some(counts) = data.get("s").and_then(Value::as_object) else {
            continue;
        };
        let mut lines = BTreeSet::new();
        for (statement, count) in counts {
            if count.as_u64().unwrap_or(1) != 0 {
                continue;
            }
            if let Some(line) = statement_map
                .get(statement)
                .and_then(|span| span.get("start"))
                .and_then(|start| start.get("line"))
                .and_then(Value::as_u64)
            {
                lines.insert(line);
            }
        }
        if !lines.is_empty() {
            failures.push(Failure {
                kind: FailureKind::Coverage,
                file: Some(normalize_path(name, root)),
                test: None,
                location: None,
                message: format!("uncovered lines {}", format_ranges(&lines)),
                actual: None,
                expected: None,
                affected: 1,
            });
        }
    }
}

fn has_coverage_threshold(output: &str) -> bool {
    output.lines().any(|line| {
        let line = strip_ansi(line).to_ascii_lowercase();
        (line.contains("coverage for ")
            && line.contains(" does not meet ")
            && line.contains(" threshold"))
            || (line.contains("uncovered ")
                && line.contains(" exceed ")
                && line.contains(" threshold"))
    })
}

fn coverage_threshold_file(line: &str, root: &Path) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let index = lower.rfind(" for ")?;
    let path = line[index + 5..]
        .trim()
        .trim_matches(['.', ',', ':', ';', '`', '"', '\'']);
    if looks_like_path(path) {
        Some(normalize_path(path, root))
    } else {
        None
    }
}

fn parse_tap(output: &str, root: &Path, failures: &mut Vec<Failure>) {
    let lines = output.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_start();
        if !line.starts_with("not ok") {
            index += 1;
            continue;
        }
        let mut name = line.split_once(" - ").map(|(_, name)| {
            let name = name.split_once(" # ").map_or(name, |(name, _)| name);
            unquote_tap(name.trim())
        });
        let mut message = String::new();
        let mut location = None;
        let mut actual = None;
        let mut expected = None;
        index += 1;
        while index < lines.len() {
            let trimmed = lines[index].trim();
            if trimmed.starts_with("not ok") || trimmed.starts_with("ok ") {
                break;
            }
            if trimmed.starts_with("message: |-") || trimmed.starts_with("message: |") {
                let mut collected = Vec::new();
                index += 1;
                while index < lines.len() && lines[index].starts_with(' ') {
                    collected.push(lines[index].trim());
                    index += 1;
                }
                message = collected.join(" ");
                continue;
            }
            if let Some(value) = tap_value(trimmed, "message") {
                let mut value = value.to_owned();
                while value.starts_with('"') && !value.ends_with('"') && index + 1 < lines.len() {
                    index += 1;
                    value.push(' ');
                    value.push_str(lines[index].trim());
                }
                message = unquote_tap(&value);
            } else if let Some(value) = tap_value(trimmed, "at") {
                location = extract_location(&unquote_tap(value), root);
            } else if let Some(value) = tap_value(trimmed, "actual") {
                actual = Some(clean_scalar(value));
            } else if let Some(value) = tap_value(trimmed, "expected") {
                expected = Some(clean_scalar(value));
            }
            index += 1;
        }
        let file = location
            .as_deref()
            .and_then(|location| location.split(':').next())
            .map(str::to_owned)
            .or_else(|| {
                name.as_deref()
                    .and_then(|name| name.split(" > ").next())
                    .filter(|name| looks_like_path(name))
                    .map(|name| normalize_path(name, root))
            });
        let test = name
            .take()
            .map(|name| name.split(" > ").last().unwrap_or(&name).trim().to_owned());
        let lower = message.to_ascii_lowercase();
        let kind = if lower.contains("timed out") || lower.contains("timeout") {
            FailureKind::Timeout
        } else if lower.contains("snapshot") {
            FailureKind::Snapshot
        } else if is_hook_message(&lower) {
            FailureKind::Suite
        } else {
            FailureKind::Assertion
        };
        failures.push(Failure {
            kind,
            file,
            test,
            location,
            message: clean_message(&message).unwrap_or_else(|| "test assertion failed".to_owned()),
            actual,
            expected,
            affected: 1,
        });
    }
}

fn tap_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)
        .and_then(|line| line.strip_prefix(':'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn unquote_tap(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('"') && value.ends_with('"') {
        serde_json::from_str::<String>(value)
            .unwrap_or_else(|_| value[1..value.len() - 1].to_owned())
    } else if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn parse_terminal_fallback(output: &str, root: &Path, failures: &mut Vec<Failure>) {
    let mut unhandled_heading = false;
    let mut pending_failure: Option<usize> = None;
    for raw in output.lines() {
        let line = strip_ansi(raw).trim().to_owned();
        let lower = line.to_ascii_lowercase();
        if is_unhandled_heading(&line) {
            unhandled_heading = true;
            pending_failure = None;
            continue;
        }
        if let Some(index) = pending_failure {
            if let Some(location) = first_repo_location(&line, root) {
                failures[index].file = location.split(':').next().map(str::to_owned);
                failures[index].location = Some(location);
                pending_failure = None;
                continue;
            }
            if line.is_empty() || line.starts_with("at ") || line.contains("node_modules/") {
                continue;
            }
            pending_failure = None;
        }
        if unhandled_heading {
            if is_terminal_noise(&line, &lower) {
                continue;
            }
            if line.starts_with("at ") || line.contains("node_modules/") {
                continue;
            }
            let location = first_repo_location(&line, root);
            let index = failures.len();
            failures.push(Failure {
                kind: FailureKind::Unhandled,
                file: location
                    .as_deref()
                    .and_then(|value| value.split(':').next().map(str::to_owned)),
                test: None,
                location,
                message: clean_message(&line)
                    .unwrap_or_else(|| "unhandled runtime error".to_owned()),
                actual: None,
                expected: None,
                affected: 1,
            });
            pending_failure = Some(index);
            unhandled_heading = false;
            continue;
        }
        if lower.contains("no test files found") || lower.contains("no tests found") {
            failures.push(Failure {
                kind: FailureKind::Discovery,
                file: None,
                test: None,
                location: None,
                message: clean_message(&line)
                    .unwrap_or_else(|| "no matching test files found".to_owned()),
                actual: None,
                expected: None,
                affected: 1,
            });
        }
        if (lower.contains("coverage for ")
            && lower.contains(" does not meet ")
            && lower.contains(" threshold"))
            || (lower.contains("uncovered ")
                && lower.contains(" exceed ")
                && lower.contains(" threshold"))
        {
            let file = coverage_threshold_file(&line, root);
            let message_source = file
                .as_ref()
                .and_then(|_| lower.rfind(" for ").map(|index| &line[..index]))
                .unwrap_or(&line);
            failures.push(Failure {
                kind: FailureKind::Coverage,
                file,
                test: None,
                location: None,
                message: clean_message(message_source)
                    .unwrap_or_else(|| "coverage threshold not met".to_owned()),
                actual: None,
                expected: None,
                affected: 1,
            });
        }
        if is_fatal_terminal_line(&lower) {
            let message =
                clean_message(&line).unwrap_or_else(|| "Vitest failed to start".to_owned());
            if failures
                .iter()
                .any(|failure| failure.message.eq_ignore_ascii_case(&message))
            {
                continue;
            }
            failures.push(Failure {
                kind: FailureKind::Runtime,
                file: extract_location(&line, root)
                    .as_deref()
                    .and_then(|value| value.split(':').next().map(str::to_owned)),
                test: None,
                location: extract_location(&line, root),
                message,
                actual: None,
                expected: None,
                affected: 1,
            });
        }
    }
}

fn is_terminal_noise(line: &str, lower: &str) -> bool {
    line.is_empty()
        || lower.starts_with("vitest caught")
        || lower.starts_with("this might cause false positive")
        || lower.starts_with("resolve unhandled errors")
        || line.chars().all(|character| "─⎯-_*".contains(character))
        || lower.starts_with("failed tests")
        || lower.starts_with("test files")
        || lower.starts_with("tests ")
}

fn is_unhandled_heading(line: &str) -> bool {
    let label = line
        .trim_matches(|character: char| character.is_whitespace() || "─⎯-_*".contains(character))
        .trim()
        .to_ascii_lowercase();
    matches!(
        label.as_str(),
        "unhandled error" | "unhandled errors" | "unhandled rejection" | "unhandled rejections"
    )
}

fn first_repo_location(value: &str, root: &Path) -> Option<String> {
    let location = extract_location(value, root)?;
    let file = location.split(':').next().unwrap_or_default();
    if file.starts_with("node_modules/") || file.contains("/.pnpm/") {
        None
    } else {
        Some(location)
    }
}

fn is_fatal_terminal_line(line: &str) -> bool {
    (line.contains("failed to load")
        || line.contains("failed to start")
        || line.contains("missing dependency")
        || line.contains("cannot find package")
        || line.contains("cannot resolve")
        || line.contains("failed to resolve")
        || line.contains("provider"))
        && !line.contains("provider coverage")
}

fn merge_failures(failures: &mut Vec<Failure>) {
    let mut evidence = Vec::new();
    for failure in failures.drain(..) {
        if let Some(existing) = evidence
            .iter_mut()
            .find(|existing: &&mut Failure| same_evidence(existing, &failure))
        {
            if existing.message == "test assertion failed"
                || failure.actual.is_some()
                || failure.expected.is_some()
            {
                existing.message = failure.message;
            }
            if existing.kind != failure.kind
                && matches!(failure.kind, FailureKind::Timeout | FailureKind::Snapshot)
            {
                existing.kind = failure.kind.clone();
            }
            if existing.location.is_none() {
                existing.location = failure.location;
            }
            if existing.actual.is_none() {
                existing.actual = failure.actual;
            }
            if existing.expected.is_none() {
                existing.expected = failure.expected;
            }
            existing.affected = existing.affected.max(failure.affected);
        } else if let Some(existing) = evidence.iter_mut().find(|existing: &&mut Failure| {
            existing.kind == FailureKind::Coverage
                && failure.kind == FailureKind::Coverage
                && existing.file.is_some()
                && existing.file == failure.file
        }) {
            if !existing.message.contains(&failure.message) {
                if existing.message.starts_with("uncovered lines ") {
                    existing.message = format!("{}; {}", failure.message, existing.message);
                } else {
                    existing.message.push_str("; ");
                    existing.message.push_str(&failure.message);
                }
            }
        } else {
            evidence.push(failure);
        }
    }

    let mut grouped = Vec::new();
    for failure in evidence {
        if let Some(existing) = grouped
            .iter_mut()
            .find(|existing: &&mut Failure| same_root_cause(existing, &failure))
        {
            existing.test = None;
            existing.affected += failure.affected;
        } else {
            grouped.push(failure);
        }
    }
    *failures = grouped;
}

fn same_evidence(existing: &Failure, failure: &Failure) -> bool {
    if existing.file != failure.file {
        return false;
    }
    if existing.kind == failure.kind
        && existing.test == failure.test
        && existing.message == failure.message
    {
        return true;
    }
    if existing.test.is_some() && existing.test == failure.test {
        return matches!(
            (&existing.kind, &failure.kind),
            (FailureKind::Assertion, FailureKind::Assertion)
                | (FailureKind::Assertion, FailureKind::Timeout)
                | (FailureKind::Timeout, FailureKind::Assertion)
                | (FailureKind::Timeout, FailureKind::Timeout)
                | (FailureKind::Snapshot, FailureKind::Snapshot)
        );
    }
    matches!(
        (&existing.kind, &failure.kind),
        (FailureKind::Suite, FailureKind::Suite) | (FailureKind::Unhandled, FailureKind::Unhandled)
    ) && existing.message == failure.message
}

fn same_root_cause(existing: &Failure, failure: &Failure) -> bool {
    let test_failure = |kind: &FailureKind| {
        matches!(
            kind,
            FailureKind::Assertion
                | FailureKind::Suite
                | FailureKind::Snapshot
                | FailureKind::Timeout
        )
    };
    test_failure(&existing.kind)
        && test_failure(&failure.kind)
        && existing.file.is_some()
        && existing.file == failure.file
        && (existing.test.is_some() || existing.affected > 1)
        && failure.test.is_some()
        && existing.message == failure.message
        && existing.location == failure.location
}

fn render_failures(failures: &[Failure]) -> String {
    let mut files = BTreeSet::new();
    for failure in failures {
        if let Some(file) = &failure.file {
            files.insert(file.clone());
        }
    }
    let mut output = format!(
        "Vitest failed: {} file(s), {} diagnostic(s)",
        files.len(),
        failures.len()
    );
    for failure in failures {
        output.push('\n');
        output.push_str("- ");
        if let Some(file) = &failure.file {
            if let Some(location) = failure
                .location
                .as_deref()
                .filter(|location| location.starts_with(&format!("{file}:")))
            {
                output.push_str(location);
            } else {
                output.push_str(file);
            }
        } else {
            output.push_str(kind_label(&failure.kind));
        }
        if let Some(test) = &failure.test {
            output.push_str(" › ");
            output.push_str(test);
        }
        if let Some(location) = failure.location.as_deref().filter(|location| {
            failure
                .file
                .as_deref()
                .is_none_or(|file| !location.starts_with(&format!("{file}:")))
        }) {
            output.push_str(" (");
            output.push_str(location);
            output.push(')');
        }
        output.push_str(" — ");
        if let (Some(actual), Some(expected)) = (&failure.actual, &failure.expected) {
            if !failure
                .message
                .to_ascii_lowercase()
                .starts_with("expected ")
            {
                output.push_str(&failure.message);
                output.push_str("; ");
            }
            output.push_str("expected ");
            output.push_str(expected);
            output.push_str("; received ");
            output.push_str(actual);
        } else {
            output.push_str(&failure.message);
        }
        if failure.affected > 1 {
            output.push_str(&format!(" (affects {} tests)", failure.affected));
        }
    }
    output
}

fn kind_label(kind: &FailureKind) -> &'static str {
    match kind {
        FailureKind::Assertion => "assertion",
        FailureKind::Suite => "suite/setup",
        FailureKind::Snapshot => "snapshot",
        FailureKind::Timeout => "timeout",
        FailureKind::Unhandled => "unhandled runtime error",
        FailureKind::Discovery => "test discovery",
        FailureKind::Coverage => "coverage",
        FailureKind::Runtime => "Vitest runtime",
    }
}

fn bounded_reason(value: &str, total: usize) -> String {
    bounded_reason_to_limit(value, total, MAX_REASON_CHARS)
}

fn bounded_reason_to_limit(value: &str, total: usize, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let lines = value.lines().collect::<Vec<_>>();
    let mut kept = Vec::new();
    let mut included = 0usize;
    let mut kept_chars = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let candidate_chars = kept_chars + usize::from(!kept.is_empty()) + line.chars().count();
        let remaining = total.saturating_sub(included + usize::from(index > 0));
        let marker = format!("… {remaining} diagnostic(s) omitted");
        if candidate_chars + 1 + marker.chars().count() > limit {
            break;
        }
        kept.push((*line).to_owned());
        kept_chars = candidate_chars;
        if index > 0 {
            included += 1;
        }
    }
    let omitted = total.saturating_sub(included);
    let marker = format!("… {omitted} diagnostic(s) omitted");
    if kept.is_empty() {
        return marker;
    }
    let mut result = kept.join("\n");
    if omitted > 0 {
        result.push('\n');
        result.push_str(&marker);
    }
    result
}

fn failure_response(stop_hook_active: bool, reason: String) -> Value {
    let reason = strip_ansi(&reason);
    let total = diagnostic_total(&reason).unwrap_or(1);
    if stop_hook_active {
        let suffix = "Retry limit reached; stopping to avoid a loop.";
        let limit = MAX_REASON_CHARS.saturating_sub(suffix.chars().count() + 1);
        let reason = bounded_reason_to_limit(&reason, total, limit);
        let message = format!("{reason}\n{suffix}");
        serde_json::json!({
            "continue": true,
            "systemMessage": message
        })
    } else {
        let reason = bounded_reason(&reason, total);
        serde_json::json!({"decision": "block", "reason": reason})
    }
}

fn diagnostic_total(value: &str) -> Option<usize> {
    value
        .lines()
        .next()?
        .split(", ")
        .find_map(|part| part.strip_suffix(" diagnostic(s)")?.parse::<usize>().ok())
}

fn warning_response(reason: String) -> Value {
    serde_json::json!({
        "continue": true,
        "systemMessage": bounded_reason(&strip_ansi(&reason), 1)
    })
}

fn normalize_path(raw: &str, root: &Path) -> String {
    let mut value = strip_ansi(raw).replace('\\', "/");
    if let Some(path) = value.strip_prefix("file://") {
        value = path.to_owned();
    }
    let root_text = root.to_string_lossy().replace('\\', "/");
    if let Some(relative) = value
        .strip_prefix(&root_text)
        .filter(|relative| relative.is_empty() || relative.starts_with('/'))
    {
        value = relative.trim_start_matches('/').to_owned();
    }
    value = value.trim_start_matches("./").to_owned();
    value
}

fn extract_location(value: &str, root: &Path) -> Option<String> {
    for token in strip_ansi(value).split_whitespace() {
        let token = token.trim_matches(|character: char| {
            matches!(
                character,
                '(' | ')' | '[' | ']' | ',' | ';' | '`' | '"' | '\''
            )
        });
        let mut pieces = token.rsplitn(3, ':');
        let column = pieces.next();
        let line = pieces.next();
        let path = pieces.next();
        if column.is_some_and(|value| value.parse::<u32>().is_ok())
            && line.is_some_and(|value| value.parse::<u32>().is_ok())
            && path.is_some_and(looks_like_path)
        {
            let path = normalize_path(path.unwrap_or_default(), root);
            if Path::new(&path).is_absolute()
                || path.starts_with("node:")
                || path.starts_with("node_modules/")
                || path.contains("/.pnpm/")
            {
                continue;
            }
            return Some(format!("{}:{}:{}", path, line.unwrap(), column.unwrap()));
        }
    }
    None
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/')
        || value.contains('\\')
        || [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts"]
            .iter()
            .any(|extension| value.ends_with(extension))
}

fn clean_message(value: &str) -> Option<String> {
    let mut lines = Vec::new();
    let value = strip_ansi(value);
    for raw in value.lines() {
        let line = raw.trim();
        let line = [
            "Error:",
            "AssertionError:",
            "TypeError:",
            "ReferenceError:",
            "SyntaxError:",
            "RangeError:",
            "ERROR:",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .unwrap_or(line)
        .trim();
        if line.is_empty()
            || line == "STACK_TRACE_ERROR"
            || line.starts_with("at ")
            || line.starts_with("at async ")
            || line.starts_with("└")
            || line.starts_with("│")
            || line.starts_with("╵")
            || line.contains("node_modules/")
        {
            continue;
        }
        lines.push(line);
        if lines.len() == 2 {
            break;
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(collapse_whitespace(&lines.join(" ")))
    }
}

fn clean_scalar(value: &str) -> String {
    let value = unquote_tap(value);
    let value = collapse_whitespace(&value);
    value.chars().take(400).collect()
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_ansi(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if character.is_control() && character != '\n' && character != '\t' {
            continue;
        }
        output.push(character);
    }
    output
}

fn format_ranges(lines: &BTreeSet<u64>) -> String {
    let mut ranges = Vec::new();
    let mut iter = lines.iter().copied();
    let Some(mut start) = iter.next() else {
        return String::new();
    };
    let mut end = start;
    for line in iter {
        if line == end + 1 {
            end = line;
        } else {
            ranges.push(if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            });
            start = line;
            end = line;
        }
    }
    ranges.push(if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    });
    ranges.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn direct_dependency_detection_ignores_transitive_and_workspace_only() {
        assert!(has_direct_vitest(&serde_json::json!({
            "devDependencies": {"vitest": "^4"}
        })));
        assert!(!has_direct_vitest(&serde_json::json!({
            "dependencies": {"vite": "^7"}
        })));
    }

    #[test]
    fn merge_preserves_unrelated_hooks_and_is_idempotent() {
        let mut value = serde_json::json!({
            "other": {"keep": true},
            "hooks": {"Stop": [
                {"matcher": "x", "hooks": [{"type": "command", "command": "echo keep"}]},
                {"hooks": [{"type": "command", "command": OWNED_COMMAND, "timeout": 1}]}
            ], "Start": [{"hooks": [{"type": "command", "command": "echo start"}]}]}
        });
        merge_hooks(&mut value).unwrap();
        let first = serde_json::to_vec(&value).unwrap();
        merge_hooks(&mut value).unwrap();
        assert_eq!(first, serde_json::to_vec(&value).unwrap());
        assert_eq!(value["other"]["keep"], true);
        assert_eq!(
            value["hooks"]["Start"][0]["hooks"][0]["command"],
            "echo start"
        );
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn tap_and_json_records_join_without_raw_stack() {
        let report = serde_json::json!({
            "success": false,
            "numTotalTestSuites": 1,
            "testResults": [{
                "name": "/repo/test/math.test.ts",
                "status": "failed",
                "assertionResults": [{
                    "fullName": "adds",
                    "status": "failed",
                    "failureMessages": ["Error: STACK_TRACE_ERROR"]
                }]
            }]
        });
        let tap = b"not ok 1 - /repo/test/math.test.ts > adds\n  message: expected 3\n  actual: 2\n  expected: 3\n  at: /repo/test/math.test.ts:2:4\n";
        let result = normalize_report(Some(&report), tap, b"", Path::new("/repo"), 1);
        assert!(result.contains("math.test.ts"));
        assert!(result.contains("expected 3; received 2"));
        assert!(!result.contains("STACK_TRACE_ERROR"));
    }

    #[test]
    fn zero_process_status_is_always_a_pass() {
        let report = serde_json::json!({
            "success": false,
            "testResults": [{
                "name": "/repo/test/bad.test.ts",
                "status": "failed",
                "message": "contradictory reporter warning"
            }]
        });
        assert!(
            normalize_report(
                Some(&report),
                b"not ok reporter warning",
                b"",
                Path::new("/repo"),
                0
            )
            .is_empty()
        );
    }

    #[test]
    fn tap_timeout_repairs_json_placeholder_and_uses_title() {
        let report = serde_json::json!({
            "success": false,
            "testResults": [{
                "name": "/repo/test/poll.test.ts",
                "status": "failed",
                "assertionResults": [{
                    "title": "waits for work",
                    "fullName": "suite waits for work",
                    "status": "failed",
                    "failureMessages": ["Error: STACK_TRACE_ERROR"]
                }]
            }]
        });
        let tap = br#"not ok 1 - "/repo/test/poll.test.ts > waits for work" # time=20ms
  message: "Test timed out in 20ms"
  at: "/repo/test/poll.test.ts:13:5"
"#;
        let result = normalize_report(Some(&report), tap, b"", Path::new("/repo"), 1);
        assert!(result.contains("waits for work"));
        assert!(result.contains("Test timed out in 20ms"));
        assert!(result.contains("test/poll.test.ts:13:5"));
        assert!(!result.contains("STACK_TRACE_ERROR"));
        assert!(!result.contains("affects 2 tests"));
    }

    #[test]
    fn repository_frame_wins_over_dependency_stack() {
        let stack = "Error: snapshot failed\n at /deps/node_modules/vitest/internal.js:4:2\n at /repo/test/view.test.ts:9:7";
        assert_eq!(
            extract_location(stack, Path::new("/repo")).as_deref(),
            Some("test/view.test.ts:9:7")
        );
    }

    #[test]
    fn tap_multiline_message_is_unquoted_and_compact() {
        let tap = br#"not ok 1 - test/poll.test.ts > waits # time=20ms
  message: "Test timed out in 20ms.
If this is long-running, configure \"testTimeout\"."
  at: "/repo/test/poll.test.ts:13:5"
"#;
        let result = normalize_report(None, tap, b"", Path::new("/repo"), 1);
        assert!(result.contains("Test timed out in 20ms. If this is long-running"));
        assert!(!result.contains("\\\"testTimeout\\\""));
    }

    #[test]
    fn repeated_root_cause_reports_affected_test_count() {
        let report = serde_json::json!({
            "success": false,
            "testResults": [{
                "name": "/repo/test/api.test.ts",
                "status": "failed",
                "assertionResults": [
                    {
                        "title": "creates order",
                        "status": "failed",
                        "failureMessages": ["Error: database fixture unavailable\n at /repo/test/api.test.ts:8:9"]
                    },
                    {
                        "title": "updates order",
                        "status": "failed",
                        "failureMessages": ["Error: database fixture unavailable\n at /repo/test/api.test.ts:8:9"]
                    }
                ]
            }]
        });
        let result = normalize_report(Some(&report), b"", b"", Path::new("/repo"), 1);
        assert!(result.contains("affects 2 tests"));
        assert_eq!(result.matches("database fixture unavailable").count(), 1);
    }

    #[test]
    fn coverage_map_requires_threshold_evidence() {
        let report = serde_json::json!({
            "success": true,
            "coverageMap": {
                "/repo/src/math.ts": {
                    "statementMap": {
                        "0": {"start": {"line": 11}},
                        "1": {"start": {"line": 12}},
                        "2": {"start": {"line": 14}}
                    },
                    "s": {"0": 0, "1": 0, "2": 1}
                }
            }
        });
        let no_threshold = normalize_report(
            Some(&report),
            b"Vitest failed for an unrelated reason",
            b"",
            Path::new("/repo"),
            1,
        );
        assert!(!no_threshold.contains("uncovered lines"));
        let threshold = normalize_report(
            Some(&report),
            b"ERROR: Coverage for lines (72%) does not meet global threshold (80%)",
            b"",
            Path::new("/repo"),
            1,
        );
        assert!(threshold.contains("uncovered lines 11-12"));
    }

    #[test]
    fn per_file_coverage_threshold_merges_uncovered_lines() {
        let report = serde_json::json!({
            "success": true,
            "coverageMap": {
                "/repo/src/math.ts": {
                    "statementMap": {"0": {"start": {"line": 11}}},
                    "s": {"0": 0}
                }
            }
        });
        let output =
            b"ERROR: Coverage for lines (72%) does not meet global threshold (80%) for src/math.ts";
        let result = normalize_report(Some(&report), output, b"", Path::new("/repo"), 1);
        assert_eq!(result.matches("src/math.ts").count(), 1);
        assert!(result.contains("uncovered lines 11"));
        assert!(result.contains("72%"));
    }

    #[test]
    fn unhandled_fallback_keeps_error_and_repository_frame_only() {
        let output = r#"⎯⎯⎯⎯⎯⎯ Unhandled Errors ⎯⎯⎯⎯⎯⎯
Vitest caught 1 unhandled error during test run.
This might cause false positive tests.
⎯⎯⎯⎯ Unhandled Rejection ⎯⎯⎯⎯
Error: UNHANDLED_BOOM
  at /repo/node_modules/pkg/index.js:4:2
  at /repo/src/jobs.ts:27:18
test('text mentions unhandled rejection', () => {})
  at node:internal/process/task_queues:104:5
"#;
        let result = normalize_report(None, output.as_bytes(), b"", Path::new("/repo"), 1);
        assert!(result.contains("UNHANDLED_BOOM"));
        assert!(result.contains("src/jobs.ts:27:18"));
        assert!(!result.contains("Vitest caught"));
        assert!(!result.contains("node_modules"));
    }

    #[test]
    fn terminal_fatal_error_does_not_duplicate_json_suite_error() {
        let report = serde_json::json!({
            "success": false,
            "testResults": [{
                "name": "/repo/test/setup.test.ts",
                "status": "failed",
                "message": "Error: Failed to load config from /repo/vitest.config.ts"
            }]
        });
        let output = b"Error: Failed to load config from /repo/vitest.config.ts";
        let result = normalize_report(Some(&report), output, b"", Path::new("/repo"), 1);
        assert_eq!(result.matches("Failed to load config").count(), 1);
    }

    #[test]
    fn retry_guard_keeps_suffix_and_omitted_marker() {
        let diagnostics = (0..100)
            .map(|index| format!("- test/{index}.test.ts — {}", "failure ".repeat(20)))
            .collect::<Vec<_>>()
            .join("\n");
        let reason = format!("Vitest failed: 100 file(s), 100 diagnostic(s)\n{diagnostics}");
        let response = failure_response(true, reason);
        let message = response["systemMessage"].as_str().unwrap();
        assert!(response["continue"].as_bool().unwrap());
        assert!(message.contains("Retry limit reached"));
        assert!(message.contains("diagnostic(s) omitted"));
        assert!(message.chars().count() <= MAX_REASON_CHARS);
    }

    #[test]
    fn coverage_ranges_are_compact_and_unicode_truncation_is_safe() {
        let lines = BTreeSet::from([11, 12, 13, 27]);
        assert_eq!(format_ranges(&lines), "11-13, 27");
        let value = "测".repeat(MAX_REASON_CHARS + 100);
        let bounded = bounded_reason(&value, 4);
        assert!(bounded.chars().count() <= MAX_REASON_CHARS);
        assert!(bounded.contains("diagnostic(s) omitted"));
    }

    #[test]
    fn bounded_reader_rejects_oversized_input() {
        let error = read_bounded(&mut Cursor::new(vec![b'x'; 5]), 4, "fixture").unwrap_err();
        assert!(error.contains("exceeds 4 bytes"));
    }
}
