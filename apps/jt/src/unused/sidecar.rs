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
    pub unknown_reasons: Vec<UnknownReason>,
    #[serde(default)]
    pub boundary_sources: Vec<BoundarySource>,
    #[serde(default)]
    pub edges: Vec<SemanticEdge>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Deserialize)]
pub struct UnknownReason {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct BoundarySource {
    pub source: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct SemanticEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub confidence: String,
    pub mode: String,
    pub provenance: String,
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
    pub top_level: bool,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn prepare_supports_vue_two_compiler_shape() {
        let project = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("temp project");
        let root = project.path();
        fs::write(root.join("package.json"), "{}").expect("package.json");
        fs::create_dir_all(root.join("node_modules/vue")).expect("vue module");
        fs::write(
            root.join("node_modules/vue/package.json"),
            r#"{"name":"vue","version":"2.7.8"}"#,
        )
        .expect("vue package");
        fs::write(
            root.join("node_modules/vue/compiler-sfc.js"),
            r#"
exports.parse = (input) => {
  if (typeof input !== 'object') return { script: null, scriptSetup: null, errors: [] }
  const open = '<script lang="ts">'
  const start = input.source.indexOf(open) + open.length
  const end = input.source.indexOf('</script>', start)
  return {
    script: {
      content: input.source.slice(start, end),
      start,
      attrs: { lang: 'ts' },
      lang: 'ts',
    },
    scriptSetup: null,
    errors: [],
  }
}
"#,
        )
        .expect("Vue 2 compiler");
        fs::create_dir(root.join("src")).expect("src");
        let source = "<template><div>中文</div></template>\n<script lang=\"ts\">\nconst value = 1\n</script>\n";
        fs::write(root.join("src/Page.vue"), source).expect("Page.vue");

        let output = prepare(root, &["src/Page.vue".to_owned()]).expect("prepare Vue 2");

        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert_eq!(output.vue_scripts.len(), 1);
        assert_eq!(output.vue_scripts[0].blocks.len(), 1);
        let block = &output.vue_scripts[0].blocks[0];
        assert_eq!(block.content, "\nconst value = 1\n");
        assert_eq!(
            block.offset,
            source.find("\nconst value").expect("script start")
        );
        assert_eq!(block.lang, "ts");
    }

    #[test]
    fn semantic_edges_include_usage_metadata() {
        let project = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("temp project");
        let root = project.path();
        fs::write(root.join("package.json"), "{}").expect("package.json");
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/used.ts"), "export function used() {}\n").expect("used.ts");
        fs::write(root.join("src/types.ts"), "export interface Shape {}\n").expect("types.ts");
        fs::write(
            root.join("src/consumer.ts"),
            "import { used } from './used';\nimport type { Shape } from './types';\nexport const value = used();\nexport type Result = Shape;\n",
        )
        .expect("consumer.ts");

        let output = references(
            root,
            &[],
            &[
                "src/used.ts".to_owned(),
                "src/types.ts".to_owned(),
                "src/consumer.ts".to_owned(),
            ],
            &[
                ReferenceCandidate {
                    id: "file::src/used.ts".to_owned(),
                    kind: "file".to_owned(),
                    path: "src/used.ts".to_owned(),
                    name: "used.ts".to_owned(),
                    start: ReferenceStart { line: 1, column: 1 },
                    top_level: true,
                },
                ReferenceCandidate {
                    id: "file::src/types.ts".to_owned(),
                    kind: "file".to_owned(),
                    path: "src/types.ts".to_owned(),
                    name: "types.ts".to_owned(),
                    start: ReferenceStart { line: 1, column: 1 },
                    top_level: true,
                },
                ReferenceCandidate {
                    id: "used-id".to_owned(),
                    kind: "function".to_owned(),
                    path: "src/used.ts".to_owned(),
                    name: "used".to_owned(),
                    start: ReferenceStart {
                        line: 1,
                        column: 17,
                    },
                    top_level: true,
                },
                ReferenceCandidate {
                    id: "value-id".to_owned(),
                    kind: "variable".to_owned(),
                    path: "src/consumer.ts".to_owned(),
                    name: "value".to_owned(),
                    start: ReferenceStart {
                        line: 2,
                        column: 14,
                    },
                    top_level: true,
                },
            ],
        )
        .expect("semantic references");

        let edge = output
            .edges
            .iter()
            .find(|edge| edge.kind == "call" && edge.target == "used-id")
            .expect("call edge");
        assert_eq!(edge.source, "file::src/consumer.ts");
        assert_eq!(edge.mode, "runtime");
        assert_eq!(edge.provenance, "typescript");
        assert!(output.edges.iter().any(|edge| {
            edge.source == "file::src/consumer.ts"
                && edge.target == "file::src/used.ts"
                && edge.kind == "import"
                && edge.mode == "runtime"
        }));
        assert!(output.edges.iter().any(|edge| {
            edge.source == "file::src/consumer.ts"
                && edge.target == "file::src/types.ts"
                && edge.kind == "reference"
                && edge.mode == "type"
        }));
    }

    #[test]
    fn semantic_helper_reports_glob_require_and_commonjs_boundaries() {
        let project = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("temp project");
        let root = project.path();
        fs::write(root.join("package.json"), "{}").expect("package.json");
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/view.ts"), "export function view() {}\n").expect("view.ts");
        fs::write(root.join("src/other.ts"), "export function other() {}\n").expect("other.ts");
        fs::write(
            root.join("src/service.ts"),
            "export function service() {}\nmodule.exports = { service };\n",
        )
        .expect("service.ts");
        fs::write(
            root.join("src/consumer.ts"),
            "import 'virtual:generated-routes';\nconst modules = import.meta.glob('./*.ts');\nconst serviceModule = require('./service');\ndeclare const runtimePath: string;\nrequire(runtimePath);\nvoid modules;\nvoid serviceModule;\n",
        )
        .expect("consumer.ts");

        let candidates = [
            ("file::src/view.ts", "file", "src/view.ts", "view.ts", 1, 1),
            (
                "file::src/service.ts",
                "file",
                "src/service.ts",
                "service.ts",
                1,
                1,
            ),
            ("service-id", "function", "src/service.ts", "service", 1, 17),
            ("other-id", "function", "src/other.ts", "other", 1, 17),
        ]
        .into_iter()
        .map(|(id, kind, path, name, line, column)| ReferenceCandidate {
            id: id.to_owned(),
            kind: kind.to_owned(),
            path: path.to_owned(),
            name: name.to_owned(),
            start: ReferenceStart { line, column },
            top_level: true,
        })
        .collect::<Vec<_>>();
        let output = references(
            root,
            &[],
            &[
                "src/view.ts".to_owned(),
                "src/service.ts".to_owned(),
                "src/other.ts".to_owned(),
                "src/consumer.ts".to_owned(),
            ],
            &candidates,
        )
        .expect("semantic references");

        assert!(output.edges.iter().any(|edge| {
            edge.source == "file::src/consumer.ts"
                && edge.target == "file::src/view.ts"
                && edge.kind == "dynamic-import"
        }));
        assert!(
            output
                .edges
                .iter()
                .any(|edge| { edge.target == "service-id" && edge.kind == "commonjs-export" })
        );
        assert!(output.edges.iter().any(|edge| {
            edge.source == "file::src/consumer.ts"
                && edge.target == "file::src/service.ts"
                && edge.kind == "require"
                && edge.mode == "runtime"
        }));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "dynamic-require-boundary")
        );
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "virtual-module-boundary")
        );
        assert!(output.boundary_sources.iter().any(|item| {
            item.source == "file::src/consumer.ts" && item.reason == "virtual-module-boundary"
        }));
    }

    #[test]
    fn semantic_require_edge_keeps_dead_function_owner() {
        let project = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("temp project");
        let root = project.path();
        fs::write(root.join("package.json"), "{}").expect("package.json");
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/target.ts"), "export function target() {}\n").expect("target.ts");
        fs::write(
            root.join("src/consumer.ts"),
            "export function dead() { require('./target'); import.meta.glob('./*.ts'); }\n",
        )
        .expect("consumer.ts");
        let output = references(
            root,
            &[],
            &["src/target.ts".to_owned(), "src/consumer.ts".to_owned()],
            &[
                ReferenceCandidate {
                    id: "file::src/target.ts".to_owned(),
                    kind: "file".to_owned(),
                    path: "src/target.ts".to_owned(),
                    name: "target.ts".to_owned(),
                    start: ReferenceStart { line: 1, column: 1 },
                    top_level: true,
                },
                ReferenceCandidate {
                    id: "dead-id".to_owned(),
                    kind: "function".to_owned(),
                    path: "src/consumer.ts".to_owned(),
                    name: "dead".to_owned(),
                    start: ReferenceStart {
                        line: 1,
                        column: 17,
                    },
                    top_level: true,
                },
            ],
        )
        .expect("semantic references");

        assert!(output.edges.iter().any(|edge| {
            edge.source == "dead-id"
                && edge.target == "file::src/target.ts"
                && edge.kind == "require"
        }));
        assert!(output.edges.iter().any(|edge| {
            edge.source == "dead-id"
                && edge.target == "file::src/target.ts"
                && edge.kind == "dynamic-import"
        }));
    }
}
