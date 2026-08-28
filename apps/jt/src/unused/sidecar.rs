use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tempfile::Builder;

const SCRIPT: &str = include_str!("../unused_semantic.cjs");
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VueBlock {
    pub content: String,
    pub offset: usize,
    pub lang: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VueScript {
    pub path: String,
    pub blocks: Vec<VueBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareOutput {
    #[serde(default)]
    pub vue_scripts: Vec<VueScript>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencesOutput {
    #[serde(default)]
    pub used_ids: Vec<String>,
    #[serde(default)]
    pub covered_ids: Vec<String>,
    #[serde(default)]
    pub unknown_ids: Vec<String>,
    #[serde(default)]
    pub edges: Vec<SemanticEdge>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub confidence: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareInput<'a> {
    mode: &'static str,
    root: &'a str,
    vue_files: &'a [String],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReferencesInput<'a> {
    mode: &'static str,
    root: &'a str,
    vue_files: &'a [String],
    source_files: &'a [String],
    candidates: &'a [ReferenceCandidate],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceCandidate {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub name: String,
    pub start: ReferenceStart,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceStart {
    pub line: usize,
    pub column: usize,
}

pub fn prepare(root: &Path, vue_files: &[String]) -> Result<PrepareOutput, String> {
    let root_text = root.to_string_lossy();
    run(
        root,
        &PrepareInput {
            mode: "prepare",
            root: &root_text,
            vue_files,
        },
    )
}

pub fn references(
    root: &Path,
    vue_files: &[String],
    source_files: &[String],
    candidates: &[ReferenceCandidate],
) -> Result<ReferencesOutput, String> {
    let root_text = root.to_string_lossy();
    run(
        root,
        &ReferencesInput {
            mode: "references",
            root: &root_text,
            vue_files,
            source_files,
            candidates,
        },
    )
}

fn run<I, O>(root: &Path, input: &I) -> Result<O, String>
where
    I: Serialize,
    O: for<'de> Deserialize<'de>,
{
    let mut script = Builder::new()
        .prefix("jt-unused-")
        .suffix(".cjs")
        .tempfile()
        .map_err(|error| format!("cannot create unused semantic helper: {error}"))?;
    script
        .write_all(SCRIPT.as_bytes())
        .map_err(|error| format!("cannot write unused semantic helper: {error}"))?;
    script
        .as_file()
        .sync_all()
        .map_err(|error| format!("cannot sync unused semantic helper: {error}"))?;

    let mut child = Command::new("node")
        .arg(script.path())
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start Node.js unused semantic helper: {error}"))?;
    let input = serde_json::to_vec(input)
        .map_err(|error| format!("cannot serialize unused semantic request: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "unused semantic helper stdin unavailable".to_owned())?
        .write_all(&input)
        .map_err(|error| format!("cannot send unused semantic request: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "unused semantic helper stdout unavailable".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "unused semantic helper stderr unavailable".to_owned())?;
    let stdout = thread::spawn(move || read_all(stdout));
    let stderr = thread::spawn(move || read_all(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for unused semantic helper: {error}"))?
        {
            break status;
        }
        if started.elapsed() >= TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "unused semantic helper timed out after {} seconds",
                TIMEOUT.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout = stdout
        .join()
        .map_err(|_| "unused semantic helper stdout reader panicked".to_owned())??;
    let stderr = stderr
        .join()
        .map_err(|_| "unused semantic helper stderr reader panicked".to_owned())??;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(format!(
            "unused semantic helper failed: {}",
            stderr.lines().next().unwrap_or("unknown error")
        ));
    }
    serde_json::from_slice(&stdout)
        .map_err(|error| format!("invalid unused semantic response: {error}"))
}

fn read_all(mut stream: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read unused semantic helper output: {error}"))?;
    Ok(bytes)
}
