mod call_graph;
mod oxc;
mod sidecar;

pub use call_graph::CallGraphArgs;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use serde::{Deserialize, Serialize};

use self::oxc::SourceBlock;
use self::sidecar::{ReferenceCandidate, ReferenceStart};

const SOURCE_EXTENSIONS: &[&str] = &["cjs", "js", "jsx", "mjs", "ts", "tsx", "mts", "cts", "vue"];
const CONFIG_PATH: &str = ".nlab/unused.config.json";
const CONFIG_VERSION: u8 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigFile {
    version: u8,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

struct ScanRoot {
    path: PathBuf,
    display: String,
    is_file: bool,
}

struct ScanConfig {
    roots: Vec<ScanRoot>,
    exclude: Gitignore,
    exclude_patterns: Vec<String>,
}

#[derive(Debug, Args)]
pub struct UnusedArgs {
    /// Project root, source directory, or source file; default: current directory
    #[arg(value_name = "PATH", default_value = ".")]
    path: PathBuf,
    /// Finding kinds; default: function,variable,file
    #[arg(long, value_enum, value_delimiter = ',')]
    kind: Vec<UnusedKind>,
    /// App: exported code still needs a project consumer. Library: exported API is ignored.
    #[arg(long, value_enum, default_value_t = AnalysisMode::App)]
    mode: AnalysisMode,
    /// Print stable machine-readable JSON
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum UnusedKind {
    Function,
    Variable,
    File,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum AnalysisMode {
    #[default]
    App,
    Library,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Finding {
    id: String,
    kind: UnusedKind,
    language: String,
    name: String,
    qualified_name: String,
    path: String,
    line: usize,
    column: usize,
    reason: String,
    reexports: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Classified {
    id: String,
    kind: UnusedKind,
    name: String,
    path: String,
    line: usize,
    column: usize,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct Diagnostic {
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    message: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    scanned_files: usize,
    scanned_symbols: usize,
    functions: usize,
    variables: usize,
    files: usize,
    ignored: usize,
    unknown: usize,
    diagnostics: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    root: String,
    scope: String,
    scan_roots: Vec<String>,
    exclude: Vec<String>,
    mode: AnalysisMode,
    findings: Vec<Finding>,
    ignored: Vec<Classified>,
    unknown: Vec<Classified>,
    diagnostics: Vec<Diagnostic>,
    summary: Summary,
}

struct Project {
    root: PathBuf,
    scope: String,
    scope_is_file: bool,
    config: ScanConfig,
}

struct Sources {
    blocks: Vec<SourceBlock>,
    contents: HashMap<String, String>,
    vue_files: Vec<String>,
    included_paths: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
}

struct Evidence {
    project: Project,
    sources: Sources,
    scan: oxc::ScanResult,
    covered: BTreeSet<String>,
    semantic_unknown: BTreeSet<String>,
    semantic_edges: Vec<sidecar::SemanticEdge>,
    parse_error_paths: BTreeSet<String>,
}

pub fn run(args: UnusedArgs) -> u8 {
    match analyze(&args) {
        Ok(report) => {
            if args.json {
                match serde_json::to_string_pretty(&report) {
                    Ok(output) => println!("{output}"),
                    Err(error) => {
                        eprintln!("error: cannot serialize unused report: {error}");
                        return 1;
                    }
                }
            } else {
                print_human(&report);
            }
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

pub fn run_call_graph(args: CallGraphArgs) -> u8 {
    call_graph::run(args)
}

fn build_evidence(path: &Path) -> Result<Evidence, String> {
    let project = find_project(path)?;
    let mut sources = collect_sources(&project)?;
    let mut scan = oxc::scan(&project.root, &sources.blocks);
    let parse_error_paths = scan
        .modules
        .iter()
        .filter(|module| module.has_parse_errors)
        .map(|module| module.path.clone())
        .collect::<BTreeSet<_>>();
    sources
        .diagnostics
        .extend(scan.diagnostics.drain(..).map(|message| Diagnostic {
            code: "oxc".to_owned(),
            path: diagnostic_path(&message),
            line: None,
            message,
        }));

    let mut references = scan
        .candidates
        .iter()
        .map(|candidate| {
            let (line, column) = sources
                .contents
                .get(&candidate.path)
                .map_or((candidate.line, candidate.column), |content| {
                    line_column(content, candidate.start)
                });
            ReferenceCandidate {
                id: candidate.id.clone(),
                kind: candidate.kind.clone(),
                path: candidate.path.clone(),
                name: candidate.name.clone(),
                start: ReferenceStart { line, column },
            }
        })
        .collect::<Vec<_>>();
    references.extend(
        scan.modules
            .iter()
            .filter(|module| sources.included_paths.contains(&module.path))
            .map(|module| ReferenceCandidate {
                id: format!("file::{}", module.path),
                kind: "file".to_owned(),
                path: module.path.clone(),
                name: Path::new(&module.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&module.path)
                    .to_owned(),
                start: ReferenceStart { line: 1, column: 1 },
            }),
    );

    let mut covered = BTreeSet::new();
    let mut semantic_unknown = BTreeSet::new();
    let mut semantic_edges = Vec::new();
    if !references.is_empty() {
        let source_files = sources.included_paths.iter().cloned().collect::<Vec<_>>();
        match sidecar::references(
            &project.root,
            &sources.vue_files,
            &source_files,
            &references,
        ) {
            Ok(output) => {
                let used = output.used_ids.into_iter().collect::<BTreeSet<_>>();
                semantic_unknown.extend(output.unknown_ids);
                covered.extend(output.covered_ids);
                semantic_edges = output.edges;
                for candidate in &mut scan.candidates {
                    candidate.local_used |= used.contains(&candidate.id);
                    if semantic_unknown.contains(&candidate.id) {
                        candidate.unknown = true;
                    } else if covered.contains(&candidate.id)
                        && !parse_error_paths.contains(&candidate.path)
                    {
                        candidate.unknown = false;
                    }
                }
                for module in &scan.modules {
                    if used.contains(&format!("file::{}", module.path)) {
                        scan.used_files.insert(module.path.clone());
                    }
                }
                sources
                    .diagnostics
                    .extend(output.diagnostics.into_iter().map(|item| {
                        Diagnostic {
                            code: item.code,
                            path: item
                                .path
                                .map(|path| relative_diagnostic_path(&project.root, &path)),
                            line: item.line,
                            message: item.message,
                        }
                    }));
            }
            Err(message) => sources.diagnostics.push(Diagnostic {
                code: "semantic-helper".to_owned(),
                path: None,
                line: None,
                message,
            }),
        }
    }
    Ok(Evidence {
        project,
        sources,
        scan,
        covered,
        semantic_unknown,
        semantic_edges,
        parse_error_paths,
    })
}

fn analyze(args: &UnusedArgs) -> Result<Report, String> {
    let Evidence {
        project,
        mut sources,
        scan,
        covered,
        semantic_unknown,
        parse_error_paths,
        ..
    } = build_evidence(&args.path)?;

    let selected = selected_kinds(&args.kind);
    let scanned_files = scan
        .modules
        .iter()
        .filter(|module| in_scope(&project, &module.path))
        .count();
    let scanned_symbols = scan
        .candidates
        .iter()
        .filter(|candidate| in_scope(&project, &candidate.path))
        .count();
    let exported_files = scan
        .modules
        .iter()
        .filter(|module| {
            module.path.ends_with(".vue")
                || !module.local_exports.is_empty()
                || !module.reexports.is_empty()
                || !module.star_exports.is_empty()
        })
        .map(|module| module.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut findings = Vec::new();
    let mut ignored = Vec::new();
    let mut unknown = Vec::new();

    for candidate in &scan.candidates {
        let kind = match candidate.kind.as_str() {
            "function" | "method" => UnusedKind::Function,
            "variable" => UnusedKind::Variable,
            _ => continue,
        };
        if !selected.contains(&kind) || !in_scope(&project, &candidate.path) {
            continue;
        }
        let (line, column) = sources
            .contents
            .get(&candidate.path)
            .map_or((candidate.line, candidate.column), |content| {
                line_column(content, candidate.start)
            });
        if let Some(reason) = structural_ignore(&candidate.path) {
            if reason == "test" {
                continue;
            }
            ignored.push(classified_candidate(candidate, kind, line, column, reason));
            continue;
        }
        if candidate.local_used {
            continue;
        }
        if candidate.name.starts_with('_') {
            ignored.push(classified_candidate(
                candidate,
                kind,
                line,
                column,
                "intentional-unused",
            ));
            continue;
        }
        if args.mode == AnalysisMode::Library && candidate.exported {
            ignored.push(classified_candidate(
                candidate,
                kind,
                line,
                column,
                "external-api",
            ));
            continue;
        }
        let vue_uncovered = candidate.path.ends_with(".vue") && !covered.contains(&candidate.id);
        let dynamic_boundary = scan.dynamic_unknown.contains(&candidate.id);
        if candidate.unknown || vue_uncovered || dynamic_boundary {
            unknown.push(classified_candidate(
                candidate,
                kind,
                line,
                column,
                if dynamic_boundary {
                    "dynamic-import-boundary"
                } else if vue_uncovered {
                    "vue-semantic-unavailable"
                } else {
                    "semantic-analysis-incomplete"
                },
            ));
            continue;
        }
        findings.push(Finding {
            id: candidate.id.clone(),
            kind,
            language: candidate.language.clone(),
            name: candidate.name.clone(),
            qualified_name: candidate.qualified_name.clone(),
            path: candidate.path.clone(),
            line,
            column,
            reason: if candidate.reexport_locations.is_empty() {
                "no-inbound-usage"
            } else {
                "reexport-only"
            }
            .to_owned(),
            reexports: candidate.reexport_locations.clone(),
        });
    }

    if selected.contains(&UnusedKind::File) {
        for module in &scan.modules {
            if !in_scope(&project, &module.path) {
                continue;
            }
            let name = Path::new(&module.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&module.path)
                .to_owned();
            let id = format!("file::{}", module.path);
            if let Some(reason) = structural_ignore(&module.path) {
                if reason == "test" {
                    continue;
                }
                ignored.push(Classified {
                    id,
                    kind: UnusedKind::File,
                    name,
                    path: module.path.clone(),
                    line: 1,
                    column: 1,
                    reason: reason.to_owned(),
                });
                continue;
            }
            if scan.used_files.contains(&module.path) {
                continue;
            }
            if args.mode == AnalysisMode::Library && exported_files.contains(module.path.as_str()) {
                ignored.push(Classified {
                    id,
                    kind: UnusedKind::File,
                    name,
                    path: module.path.clone(),
                    line: 1,
                    column: 1,
                    reason: "external-api".to_owned(),
                });
                continue;
            }
            let semantic_unavailable = module.path.ends_with(".vue")
                && !covered.contains(&format!("file::{}", module.path));
            let semantic_incomplete = semantic_unknown.contains(&id);
            if parse_error_paths.contains(&module.path)
                || semantic_unavailable
                || semantic_incomplete
            {
                unknown.push(Classified {
                    id,
                    kind: UnusedKind::File,
                    name,
                    path: module.path.clone(),
                    line: 1,
                    column: 1,
                    reason: if semantic_incomplete {
                        "semantic-analysis-incomplete"
                    } else if semantic_unavailable {
                        "vue-semantic-unavailable"
                    } else {
                        "parse-errors"
                    }
                    .to_owned(),
                });
                continue;
            }
            let reexports = scan
                .file_reexports
                .get(&module.path)
                .cloned()
                .unwrap_or_default();
            let reexport_only = !reexports.is_empty();
            findings.push(Finding {
                id,
                kind: UnusedKind::File,
                language: language_for_path(&module.path).to_owned(),
                name,
                qualified_name: module.path.clone(),
                path: module.path.clone(),
                line: 1,
                column: 1,
                reason: if reexport_only {
                    "reexport-only"
                } else {
                    "no-inbound-usage"
                }
                .to_owned(),
                reexports,
            });
        }
    }

    sort_results(&mut findings, &mut ignored, &mut unknown);
    sources.diagnostics.retain(|diagnostic| {
        diagnostic
            .path
            .as_deref()
            .is_none_or(|path| in_scope(&project, path))
    });
    sources.diagnostics.sort_by(|left, right| {
        (&left.path, left.line, &left.code, &left.message).cmp(&(
            &right.path,
            right.line,
            &right.code,
            &right.message,
        ))
    });
    sources.diagnostics.dedup_by(|left, right| {
        left.code == right.code
            && left.path == right.path
            && left.line == right.line
            && left.message == right.message
    });
    let summary = summarize(
        scanned_files,
        scanned_symbols,
        &findings,
        &ignored,
        &unknown,
        &sources.diagnostics,
    );
    Ok(Report {
        root: project.root.to_string_lossy().into_owned(),
        scope: if project.scope.is_empty() {
            ".".to_owned()
        } else {
            project.scope
        },
        scan_roots: project
            .config
            .roots
            .iter()
            .map(|root| root.display.clone())
            .collect(),
        exclude: project.config.exclude_patterns,
        mode: args.mode,
        findings,
        ignored,
        unknown,
        diagnostics: sources.diagnostics,
        summary,
    })
}

fn find_project(path: &Path) -> Result<Project, String> {
    let requested = fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    let scope_is_file = requested.is_file();
    let mut directory = if scope_is_file {
        requested
            .parent()
            .ok_or_else(|| format!("cannot inspect {}", requested.display()))?
            .to_path_buf()
    } else {
        requested.clone()
    };
    let root = loop {
        if ["package.json", "tsconfig.json", "jsconfig.json"]
            .iter()
            .any(|name| directory.join(name).is_file())
        {
            break directory;
        }
        let Some(parent) = directory.parent() else {
            return Err(format!(
                "not a JavaScript/TypeScript/Vue project: {} (package.json, tsconfig.json, or jsconfig.json not found)",
                requested.display()
            ));
        };
        directory = parent.to_path_buf();
    };
    let scope = requested
        .strip_prefix(&root)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");
    let config = load_config(&root)?;
    if !scope.is_empty() {
        let scope_path = Path::new(&scope);
        if config.is_excluded(&root, scope_path, !scope_is_file) {
            return Err(format!(
                "requested scope is excluded by {CONFIG_PATH}: {scope}"
            ));
        }
        if !config.intersects(scope_path, scope_is_file) {
            return Err(format!(
                "requested scope is outside configured roots in {CONFIG_PATH}: {scope}"
            ));
        }
    }
    Ok(Project {
        root,
        scope,
        scope_is_file,
        config,
    })
}

impl ScanConfig {
    fn contains(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| {
            if root.path.as_os_str().is_empty() {
                true
            } else if root.is_file {
                path == root.path
            } else {
                path == root.path || path.starts_with(&root.path)
            }
        })
    }

    fn intersects(&self, scope: &Path, scope_is_file: bool) -> bool {
        if scope_is_file {
            return self.contains(scope);
        }
        self.contains(scope)
            || self.roots.iter().any(|root| {
                root.path.as_os_str().is_empty()
                    || root.path == scope
                    || root.path.starts_with(scope)
            })
    }

    fn is_excluded(&self, project_root: &Path, path: &Path, is_dir: bool) -> bool {
        self.exclude
            .matched_path_or_any_parents(project_root.join(path), is_dir)
            .is_ignore()
    }

    fn includes(&self, project_root: &Path, path: &Path, is_dir: bool) -> bool {
        self.contains(path) && !self.is_excluded(project_root, path, is_dir)
    }
}

fn load_config(root: &Path) -> Result<ScanConfig, String> {
    let config_path = root.join(CONFIG_PATH);
    let source = match fs::symlink_metadata(&config_path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(format!("{CONFIG_PATH} must be a regular file"));
            }
            let canonical = fs::canonicalize(&config_path)
                .map_err(|error| format!("cannot resolve {CONFIG_PATH}: {error}"))?;
            if !canonical.starts_with(root) {
                return Err(format!("{CONFIG_PATH} escapes the project root"));
            }
            Some(
                fs::read_to_string(&canonical)
                    .map_err(|error| format!("cannot read {CONFIG_PATH}: {error}"))?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot inspect {CONFIG_PATH}: {error}")),
    };
    let file = match source {
        Some(source) => serde_json::from_str::<ConfigFile>(&source)
            .map_err(|error| format!("invalid {CONFIG_PATH}: {error}"))?,
        None => ConfigFile {
            version: CONFIG_VERSION,
            roots: Vec::new(),
            exclude: Vec::new(),
        },
    };
    if file.version != CONFIG_VERSION {
        return Err(format!(
            "unsupported {CONFIG_PATH} version {}; expected {CONFIG_VERSION}",
            file.version
        ));
    }

    let root_values = if file.roots.is_empty() {
        vec![".".to_owned()]
    } else {
        file.roots
    };
    let mut roots = BTreeMap::<PathBuf, ScanRoot>::new();
    for value in root_values {
        let relative = config_relative_path(&value, "roots")?;
        let requested = root.join(&relative);
        let canonical = fs::canonicalize(&requested)
            .map_err(|error| format!("invalid {CONFIG_PATH} root {value:?}: {error}"))?;
        if !canonical.starts_with(root) {
            return Err(format!(
                "invalid {CONFIG_PATH} root {value:?}: path escapes the project root"
            ));
        }
        if canonical != requested {
            return Err(format!(
                "invalid {CONFIG_PATH} root {value:?}: symlinked roots are unsupported"
            ));
        }
        let relative = canonical
            .strip_prefix(root)
            .expect("validated config root")
            .to_path_buf();
        let display = if relative.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            relative.to_string_lossy().replace('\\', "/")
        };
        roots.insert(
            relative.clone(),
            ScanRoot {
                path: relative,
                display,
                is_file: canonical.is_file(),
            },
        );
    }

    let mut exclude_patterns = file.exclude;
    exclude_patterns.sort();
    exclude_patterns.dedup();
    let mut builder = GitignoreBuilder::new(root);
    for pattern in &exclude_patterns {
        validate_exclude_pattern(pattern)?;
        builder
            .add_line(None, pattern)
            .map_err(|error| format!("invalid {CONFIG_PATH} exclude {pattern:?}: {error}"))?;
    }
    let exclude = builder
        .build()
        .map_err(|error| format!("invalid {CONFIG_PATH}: {error}"))?;
    Ok(ScanConfig {
        roots: roots.into_values().collect(),
        exclude,
        exclude_patterns,
    })
}

fn config_relative_path(value: &str, field: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.trim() != value || value.contains('\\') {
        return Err(format!(
            "invalid {CONFIG_PATH} {field} path {value:?}; use a non-empty root-relative path with / separators"
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "invalid {CONFIG_PATH} {field} path {value:?}; absolute paths and .. are forbidden"
        ));
    }
    let normalized = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect::<PathBuf>();
    Ok(normalized)
}

fn validate_exclude_pattern(pattern: &str) -> Result<(), String> {
    if pattern.starts_with('!') || pattern.starts_with('#') {
        return Err(format!(
            "invalid {CONFIG_PATH} exclude {pattern:?}; negation and comments are unsupported"
        ));
    }
    config_relative_path(pattern, "exclude").map(|_| ())
}

fn collect_sources(project: &Project) -> Result<Sources, String> {
    let root = &project.root;
    let mut contents = HashMap::new();
    let mut vue_files = Vec::new();
    let mut included_paths = BTreeSet::new();
    let mut placeholder_paths = Vec::new();
    let mut diagnostics = Vec::new();
    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .follow_links(false)
        .filter_entry(|entry| !excluded_directory(entry.path()))
        .build();
    for entry in walker {
        let entry = entry.map_err(|error| format!("cannot walk project: {error}"))?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || !is_source(entry.path()) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        paths.push((relative, entry.into_path()));
    }
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    for (relative, path) in paths {
        if !project.config.includes(root, Path::new(&relative), false) {
            placeholder_paths.push(relative);
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(content) => {
                included_paths.insert(relative.clone());
                if relative.ends_with(".vue") {
                    vue_files.push(relative.clone());
                }
                contents.insert(relative, content);
            }
            Err(error) => diagnostics.push(Diagnostic {
                code: "read-source".to_owned(),
                path: Some(relative),
                line: None,
                message: error.to_string(),
            }),
        }
    }

    let mut blocks = contents
        .iter()
        .filter(|(path, _)| !path.ends_with(".vue"))
        .map(|(path, content)| {
            SourceBlock::new(path.clone(), content.clone(), 0, language_for_path(path))
        })
        .collect::<Vec<_>>();
    blocks.extend(
        vue_files
            .iter()
            .map(|path| SourceBlock::new(path.clone(), "", 0, "ts")),
    );
    blocks.extend(placeholder_paths.into_iter().map(|path| {
        let language = language_for_path(&path);
        SourceBlock::new(path, "", 0, language)
    }));
    if !vue_files.is_empty() {
        match sidecar::prepare(root, &vue_files) {
            Ok(output) => {
                diagnostics.extend(output.diagnostics.into_iter().map(|item| Diagnostic {
                    code: item.code,
                    path: item.path.map(|path| relative_diagnostic_path(root, &path)),
                    line: item.line,
                    message: item.message,
                }));
                blocks.extend(output.vue_scripts.into_iter().flat_map(|script| {
                    script.blocks.into_iter().map(move |block| {
                        SourceBlock::new(
                            script.path.clone(),
                            block.content,
                            block.offset,
                            block.lang,
                        )
                    })
                }));
            }
            Err(message) => {
                diagnostics.push(Diagnostic {
                    code: "vue-prepare".to_owned(),
                    path: None,
                    line: None,
                    message,
                });
            }
        }
    }
    blocks.sort_by(|left, right| {
        (left.path.as_str(), left.offset).cmp(&(right.path.as_str(), right.offset))
    });
    Ok(Sources {
        blocks,
        contents,
        vue_files,
        included_paths,
        diagnostics,
    })
}

fn selected_kinds(kinds: &[UnusedKind]) -> BTreeSet<UnusedKind> {
    if kinds.is_empty() {
        [UnusedKind::Function, UnusedKind::Variable, UnusedKind::File]
            .into_iter()
            .collect()
    } else {
        kinds.iter().copied().collect()
    }
}

fn in_scope(project: &Project, path: &str) -> bool {
    project
        .config
        .includes(&project.root, Path::new(path), false)
        && (project.scope.is_empty()
            || if project.scope_is_file {
                path == project.scope
            } else {
                path == project.scope || path.starts_with(&format!("{}/", project.scope))
            })
}

fn structural_ignore(path: &str) -> Option<&'static str> {
    if is_test_path(path) {
        Some("test")
    } else if is_declaration_file(path) {
        Some("type-declaration")
    } else if Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("main"))
    {
        Some("entrypoint")
    } else {
        None
    }
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let base = Path::new(&lower)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    lower
        .split('/')
        .any(|part| matches!(part, "test" | "tests" | "__tests__" | "e2e" | "cypress"))
        || base.contains(".test.")
        || base.contains(".spec.")
        || base.contains(".e2e.")
        || base.starts_with("test_")
        || base
            .split_once('.')
            .is_some_and(|(stem, _)| stem.ends_with("_test"))
}

fn is_declaration_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".d.ts", ".d.tsx", ".d.mts", ".d.cts"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn excluded_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | ".codegraph" | "node_modules" | "dist" | "coverage"
            )
        })
}

fn language_for_path(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("ts" | "mts" | "cts") => "typescript",
        Some("tsx") => "tsx",
        Some("jsx") => "jsx",
        Some("vue") => "vue",
        _ => "javascript",
    }
}

fn line_column(content: &str, start: usize) -> (usize, usize) {
    let mut start = start.min(content.len());
    while !content.is_char_boundary(start) {
        start -= 1;
    }
    let prefix = &content[..start];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
    let current_line = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    let column = current_line.encode_utf16().count() + 1;
    (line, column)
}

fn classified_candidate(
    candidate: &oxc::Candidate,
    kind: UnusedKind,
    line: usize,
    column: usize,
    reason: &str,
) -> Classified {
    Classified {
        id: candidate.id.clone(),
        kind,
        name: candidate.name.clone(),
        path: candidate.path.clone(),
        line,
        column,
        reason: reason.to_owned(),
    }
}

fn sort_results(findings: &mut [Finding], ignored: &mut [Classified], unknown: &mut [Classified]) {
    findings.sort_by(|left, right| {
        (&left.path, left.line, left.column, left.kind, &left.name).cmp(&(
            &right.path,
            right.line,
            right.column,
            right.kind,
            &right.name,
        ))
    });
    let sort_classified = |left: &Classified, right: &Classified| {
        (&left.path, left.line, left.column, left.kind, &left.name).cmp(&(
            &right.path,
            right.line,
            right.column,
            right.kind,
            &right.name,
        ))
    };
    ignored.sort_by(sort_classified);
    unknown.sort_by(sort_classified);
}

fn summarize(
    scanned_files: usize,
    scanned_symbols: usize,
    findings: &[Finding],
    ignored: &[Classified],
    unknown: &[Classified],
    diagnostics: &[Diagnostic],
) -> Summary {
    Summary {
        scanned_files,
        scanned_symbols,
        functions: findings
            .iter()
            .filter(|finding| finding.kind == UnusedKind::Function)
            .count(),
        variables: findings
            .iter()
            .filter(|finding| finding.kind == UnusedKind::Variable)
            .count(),
        files: findings
            .iter()
            .filter(|finding| finding.kind == UnusedKind::File)
            .count(),
        ignored: ignored.len(),
        unknown: unknown.len(),
        diagnostics: diagnostics.len(),
    }
}

fn diagnostic_path(message: &str) -> Option<String> {
    let path = message.split_once(':')?.0;
    is_source(Path::new(path)).then(|| path.to_owned())
}

fn relative_diagnostic_path(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn print_human(report: &Report) {
    let mut grouped = BTreeMap::<&str, Vec<&Finding>>::new();
    for finding in &report.findings {
        grouped.entry(&finding.path).or_default().push(finding);
    }
    for (path, findings) in grouped {
        println!("{path}");
        for finding in findings {
            println!(
                "  {}:{} {} {} [{}]",
                finding.line,
                finding.column,
                kind_name(finding.kind),
                finding.name,
                finding.reason
            );
        }
    }
    if !report.unknown.is_empty() {
        println!("unknown");
        for item in &report.unknown {
            println!(
                "  {}:{}:{} {} {} [{}]",
                item.path,
                item.line,
                item.column,
                kind_name(item.kind),
                item.name,
                item.reason
            );
        }
    }
    if !report.diagnostics.is_empty() {
        println!("diagnostics");
        for item in &report.diagnostics {
            let location = match (&item.path, item.line) {
                (Some(path), Some(line)) => format!("{path}:{line}"),
                (Some(path), None) => path.clone(),
                (None, _) => "project".to_owned(),
            };
            println!("  {location} {} {}", item.code, item.message);
        }
    }
    let summary = &report.summary;
    println!(
        "scanned {} {}, {} {}",
        summary.scanned_files,
        plural(summary.scanned_files, "file", "files"),
        summary.scanned_symbols,
        plural(summary.scanned_symbols, "symbol", "symbols")
    );
    println!(
        "{} unused: {} {}, {} {}, {} {}",
        report.findings.len(),
        summary.functions,
        plural(summary.functions, "function", "functions"),
        summary.variables,
        plural(summary.variables, "variable", "variables"),
        summary.files,
        plural(summary.files, "file", "files")
    );
    println!(
        "{} ignored, {} unknown, {} diagnostics",
        summary.ignored, summary.unknown, summary.diagnostics
    );
}

fn kind_name(kind: UnusedKind) -> &'static str {
    match kind {
        UnusedKind::Function => "function",
        UnusedKind::Variable => "variable",
        UnusedKind::File => "file",
    }
}

fn plural(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_paths_cover_entrypoints_declarations_and_tests() {
        assert_eq!(structural_ignore("src/main.ts"), Some("entrypoint"));
        assert_eq!(
            structural_ignore("src/components.d.ts"),
            Some("type-declaration")
        );
        assert_eq!(structural_ignore("src/foo.spec.ts"), Some("test"));
        assert_eq!(structural_ignore("src/__tests__/foo.ts"), Some("test"));
        assert_eq!(structural_ignore("src/foo.ts"), None);
    }

    #[test]
    fn source_positions_are_one_based() {
        let source = "const 一 = 1;\nfunction foo() {}";
        assert_eq!(
            line_column(source, source.find("function").unwrap()),
            (2, 1)
        );
        assert_eq!(line_column(source, source.find('一').unwrap()), (1, 7));
    }
}
