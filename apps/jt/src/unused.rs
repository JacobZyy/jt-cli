mod call_graph;
mod oxc;
mod policy;
mod sidecar;

pub use call_graph::CallGraphArgs;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::oxc::SourceBlock;
use self::sidecar::{ReferenceCandidate, ReferenceStart};

const SOURCE_EXTENSIONS: &[&str] = &["cjs", "js", "jsx", "mjs", "ts", "tsx", "mts", "cts", "vue"];
const CONFIG_PATH: &str = ".nlab/unused.config.json";
const CONFIG_VERSION: u8 = 2;
const LEGACY_CONFIG_VERSION: u8 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigFile {
    version: u8,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    entrypoints: Option<Vec<String>>,
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
    entrypoints: BTreeSet<String>,
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
    /// App: exports need a consumer. Library: package public-entry closure is external API.
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
    consumer_paths: BTreeSet<String>,
    runtime_consumer_roots: BTreeSet<String>,
    type_consumer_roots: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
}

struct Evidence {
    project: Project,
    sources: Sources,
    scan: oxc::ScanResult,
    covered: BTreeSet<String>,
    semantic_unknown: BTreeSet<String>,
    semantic_unknown_reasons: BTreeMap<String, String>,
    semantic_boundaries: BTreeMap<String, String>,
    semantic_edges: Vec<sidecar::SemanticEdge>,
    parse_error_paths: BTreeSet<String>,
    entrypoints: BTreeSet<String>,
    public_entrypoints: BTreeSet<String>,
    entrypoints_complete: bool,
    framework_incomplete: bool,
}

struct EntrypointDiscovery {
    files: BTreeSet<String>,
    public_files: BTreeSet<String>,
    complete: bool,
    framework_incomplete: bool,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Default)]
struct CoverageMap {
    candidates: BTreeMap<String, String>,
    files: BTreeMap<String, String>,
}

impl CoverageMap {
    fn mark_candidate(&mut self, id: &str, reason: &str) {
        self.candidates
            .entry(id.to_owned())
            .or_insert_with(|| reason.to_owned());
    }

    fn mark_file(&mut self, path: &str, reason: &str) {
        self.files
            .entry(path.to_owned())
            .or_insert_with(|| reason.to_owned());
    }

    fn candidate_reason(&self, id: &str) -> Option<&str> {
        self.candidates.get(id).map(String::as_str)
    }

    fn file_reason(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }
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

fn build_evidence(path: &Path, include_entrypoints: bool) -> Result<Evidence, String> {
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
                top_level: candidate.top_level,
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
                top_level: true,
            }),
    );

    let mut covered = BTreeSet::new();
    let mut semantic_unknown = BTreeSet::new();
    let mut semantic_unknown_reasons = BTreeMap::new();
    let mut semantic_boundaries = BTreeMap::new();
    let mut semantic_edges = Vec::new();
    if !references.is_empty() {
        let source_files = sources.consumer_paths.iter().cloned().collect::<Vec<_>>();
        match sidecar::references(
            &project.root,
            &sources.vue_files,
            &source_files,
            &references,
        ) {
            Ok(output) => {
                let used = output.used_ids.into_iter().collect::<BTreeSet<_>>();
                semantic_unknown.extend(output.unknown_ids);
                semantic_unknown_reasons.extend(
                    output
                        .unknown_reasons
                        .into_iter()
                        .map(|item| (item.id, item.reason)),
                );
                semantic_boundaries.extend(
                    output
                        .boundary_sources
                        .into_iter()
                        .map(|item| (item.source, item.reason)),
                );
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
    let entrypoints = if include_entrypoints {
        discover_entrypoints(&project, &sources.included_paths)?
    } else {
        EntrypointDiscovery {
            files: BTreeSet::new(),
            public_files: BTreeSet::new(),
            complete: false,
            framework_incomplete: false,
            diagnostics: Vec::new(),
        }
    };
    sources.diagnostics.extend(entrypoints.diagnostics);
    Ok(Evidence {
        project,
        sources,
        scan,
        covered,
        semantic_unknown,
        semantic_unknown_reasons,
        semantic_boundaries,
        semantic_edges,
        parse_error_paths,
        entrypoints: entrypoints.files,
        public_entrypoints: entrypoints.public_files,
        entrypoints_complete: entrypoints.complete,
        framework_incomplete: entrypoints.framework_incomplete,
    })
}

fn build_coverage(evidence: &Evidence) -> CoverageMap {
    let mut coverage = CoverageMap::default();
    if evidence.framework_incomplete {
        for path in &evidence.sources.included_paths {
            coverage.mark_file(path, "framework-semantic-incomplete");
        }
        for candidate in evidence
            .scan
            .candidates
            .iter()
            .filter(|candidate| candidate.exported)
        {
            coverage.mark_candidate(&candidate.id, "framework-semantic-incomplete");
        }
    }
    for path in &evidence.parse_error_paths {
        coverage.mark_file(path, "parse-errors");
        for candidate in evidence
            .scan
            .candidates
            .iter()
            .filter(|candidate| candidate.path == *path)
        {
            coverage.mark_candidate(&candidate.id, "parse-errors");
        }
    }
    for path in &evidence.scan.unknown_files {
        coverage.mark_file(path, "module-resolution-incomplete");
        for candidate in evidence
            .scan
            .candidates
            .iter()
            .filter(|candidate| candidate.path == *path)
        {
            coverage.mark_candidate(&candidate.id, "module-resolution-incomplete");
        }
    }
    for candidate in &evidence.scan.candidates {
        if let Some(reason) = candidate.coverage_reason.as_deref() {
            coverage.mark_candidate(&candidate.id, reason);
        }
        if candidate.initializer_effect == "unknown" {
            coverage.mark_candidate(&candidate.id, "initializer-effect-unknown");
        }
        if evidence.semantic_unknown.contains(&candidate.id) {
            coverage.mark_candidate(
                &candidate.id,
                evidence
                    .semantic_unknown_reasons
                    .get(&candidate.id)
                    .map_or("semantic-analysis-incomplete", String::as_str),
            );
        } else if candidate.unknown {
            coverage.mark_candidate(&candidate.id, "semantic-analysis-incomplete");
        }
        if candidate.path.ends_with(".vue") && !evidence.covered.contains(&candidate.id) {
            coverage.mark_candidate(&candidate.id, "vue-semantic-unavailable");
        }
        if evidence.scan.dynamic_unknown.contains(&candidate.id) {
            coverage.mark_candidate(&candidate.id, "dynamic-import-boundary");
        }
    }
    for id in &evidence.semantic_unknown {
        if let Some(path) = id.strip_prefix("file::") {
            coverage.mark_file(
                path,
                evidence
                    .semantic_unknown_reasons
                    .get(id)
                    .map_or("semantic-analysis-incomplete", String::as_str),
            );
        }
    }
    for module in &evidence.scan.modules {
        if module.path.ends_with(".vue")
            && evidence.sources.included_paths.contains(&module.path)
            && !evidence.covered.contains(&format!("file::{}", module.path))
        {
            coverage.mark_file(&module.path, "vue-semantic-unavailable");
        }
    }
    coverage
}

fn library_public_surface(
    scan: &oxc::ScanResult,
    semantic_edges: &[sidecar::SemanticEdge],
    entrypoints: &BTreeSet<String>,
) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let mut files = entrypoints.clone();
    let mut queue = entrypoints.iter().cloned().collect::<VecDeque<_>>();
    let mut reexports = HashMap::<String, Vec<String>>::new();
    for edge in &scan.edges {
        if !matches!(edge.kind.as_str(), "reexport" | "re-export" | "reexports") {
            continue;
        }
        let (Some(source), Some(target)) = (
            edge.source.strip_prefix("file::"),
            edge.target.strip_prefix("file::"),
        ) else {
            continue;
        };
        reexports
            .entry(source.to_owned())
            .or_default()
            .push(target.to_owned());
    }
    while let Some(source) = queue.pop_front() {
        for target in reexports.get(&source).into_iter().flatten() {
            if files.insert(target.clone()) {
                queue.push_back(target.clone());
            }
        }
    }

    let mut candidates = entrypoints
        .iter()
        .filter_map(|path| scan.public_exports.get(path))
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let candidate_ids = scan
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect::<BTreeSet<_>>();
    let owner_ids = scan
        .execution_owners
        .iter()
        .map(|owner| owner.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut owners = BTreeSet::new();
    for edge in &scan.edges {
        if edge.kind != "commonjs-export"
            || !edge
                .source
                .strip_prefix("file::")
                .is_some_and(|path| files.contains(path))
        {
            continue;
        }
        if candidate_ids.contains(edge.target.as_str()) {
            candidates.insert(edge.target.clone());
        } else if owner_ids.contains(edge.target.as_str()) {
            owners.insert(edge.target.clone());
        }
    }
    for edge in semantic_edges {
        if edge.kind == "commonjs-export"
            && edge
                .source
                .strip_prefix("file::")
                .is_some_and(|path| files.contains(path))
        {
            candidates.insert(edge.target.clone());
        }
    }
    (files, candidates, owners)
}

fn analyze(args: &UnusedArgs) -> Result<Report, String> {
    let evidence = build_evidence(&args.path, true)?;
    let mut coverage = build_coverage(&evidence);
    let Evidence {
        project,
        mut sources,
        scan,
        semantic_edges,
        semantic_boundaries,
        entrypoints,
        public_entrypoints,
        entrypoints_complete,
        ..
    } = evidence;

    let (public_files, public_candidates, public_owners) = if args.mode == AnalysisMode::Library {
        library_public_surface(&scan, &semantic_edges, &public_entrypoints)
    } else {
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new())
    };
    let (analysis_roots, analysis_entrypoints_complete) = if args.mode == AnalysisMode::Library {
        let mut roots = public_entrypoints.clone();
        roots.extend(public_candidates.iter().cloned());
        roots.extend(public_owners.iter().cloned());
        (roots, !public_entrypoints.is_empty())
    } else {
        (entrypoints.clone(), entrypoints_complete)
    };
    if args.mode == AnalysisMode::Library && public_entrypoints.is_empty() {
        sources.diagnostics.push(Diagnostic {
            code: "entrypoint-coverage-incomplete".to_owned(),
            path: Some("package.json".to_owned()),
            line: None,
            message: "cannot prove a package public entrypoint for library analysis".to_owned(),
        });
    }
    if !analysis_entrypoints_complete {
        for path in &sources.included_paths {
            coverage.mark_file(path, "entrypoint-coverage-incomplete");
        }
        for candidate in scan
            .candidates
            .iter()
            .filter(|candidate| candidate.exported)
        {
            coverage.mark_candidate(&candidate.id, "entrypoint-coverage-incomplete");
        }
    }
    let reachability = policy::compute(
        &scan,
        &semantic_edges,
        &analysis_roots,
        analysis_entrypoints_complete,
        &sources.runtime_consumer_roots,
        &sources.type_consumer_roots,
    );
    for (source, reason) in scan.coverage_boundaries.iter().chain(&semantic_boundaries) {
        let reachable = source
            .strip_prefix("file::")
            .is_some_and(|path| reachability.runtime_used_files.contains(path))
            || reachability.active_runtime_owners.contains(source);
        if reachable {
            for path in &sources.included_paths {
                coverage.mark_file(path, reason);
            }
            for candidate in &scan.candidates {
                coverage.mark_candidate(&candidate.id, reason);
            }
        }
    }
    for (path, locations) in &scan.file_reexports {
        let runtime_loaded = locations.iter().any(|location| {
            !location.type_only && reachability.runtime_used_files.contains(&location.source)
        });
        if runtime_loaded
            && !matches!(
                scan.file_effects.get(path).map(String::as_str),
                Some("none" | "side-effect-free" | "side-effectful")
            )
        {
            coverage.mark_file(path, "top-level-side-effect-unknown");
            for candidate in scan
                .candidates
                .iter()
                .filter(|candidate| candidate.path == *path)
            {
                coverage.mark_candidate(&candidate.id, "top-level-side-effect-unknown");
            }
        }
    }

    let selected = selected_kinds(&args.kind);
    let scanned_files = scan
        .modules
        .iter()
        .filter(|module| {
            sources.included_paths.contains(&module.path) && in_scope(&project, &module.path)
        })
        .count();
    let scanned_symbols = scan
        .candidates
        .iter()
        .filter(|candidate| in_scope(&project, &candidate.path))
        .count();
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
        if args.mode == AnalysisMode::Library && public_candidates.contains(&candidate.id) {
            ignored.push(classified_candidate(
                candidate,
                kind,
                line,
                column,
                "external-api",
            ));
            continue;
        }
        if reachability.used_candidates.contains(&candidate.id) {
            continue;
        }
        if let Some(reason) = coverage.candidate_reason(&candidate.id) {
            unknown.push(classified_candidate(candidate, kind, line, column, reason));
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
            reason: if candidate.reexport_locations.is_empty()
                && !reachability.used_files.contains(&candidate.path)
            {
                "unreachable-from-entrypoint"
            } else if candidate.initializer_effect == "side-effectful" {
                "unused-binding-side-effectful-initializer"
            } else if candidate.reexport_locations.is_empty() {
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
            if !sources.included_paths.contains(&module.path) || !in_scope(&project, &module.path) {
                continue;
            }
            let name = Path::new(&module.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&module.path)
                .to_owned();
            let id = format!("file::{}", module.path);
            if entrypoints.contains(&module.path) {
                ignored.push(Classified {
                    id,
                    kind: UnusedKind::File,
                    name,
                    path: module.path.clone(),
                    line: 1,
                    column: 1,
                    reason: "entrypoint".to_owned(),
                });
                continue;
            }
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
            if args.mode == AnalysisMode::Library && public_files.contains(&module.path) {
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
            if reachability.used_files.contains(&module.path) {
                continue;
            }
            if let Some(reason) = coverage.file_reason(&module.path) {
                unknown.push(Classified {
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
            let reexport_evidence = scan
                .file_reexports
                .get(&module.path)
                .cloned()
                .unwrap_or_default();
            let reexport_only = !reexport_evidence.is_empty();
            let reexports = reexport_evidence
                .iter()
                .map(oxc::ReexportLocation::display)
                .collect();
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
                    "unreachable-from-entrypoint"
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
            entrypoints: None,
            exclude: Vec::new(),
        },
    };
    if !matches!(file.version, LEGACY_CONFIG_VERSION | CONFIG_VERSION) {
        return Err(format!(
            "unsupported {CONFIG_PATH} version {}; expected {LEGACY_CONFIG_VERSION} or {CONFIG_VERSION}",
            file.version,
        ));
    }
    if file.version == LEGACY_CONFIG_VERSION && file.entrypoints.is_some() {
        return Err(format!(
            "invalid {CONFIG_PATH}: entrypoints require version {CONFIG_VERSION}"
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

    let mut entrypoints = BTreeSet::new();
    for value in file.entrypoints.unwrap_or_default() {
        let relative = config_relative_path(&value, "entrypoints")?;
        let requested = root.join(&relative);
        let canonical = fs::canonicalize(&requested)
            .map_err(|error| format!("invalid {CONFIG_PATH} entrypoint {value:?}: {error}"))?;
        if !canonical.starts_with(root)
            || canonical != requested
            || !canonical.is_file()
            || !is_source(&canonical)
        {
            return Err(format!(
                "invalid {CONFIG_PATH} entrypoint {value:?}: expected a supported regular non-symlinked source file inside the project root"
            ));
        }
        let relative = canonical
            .strip_prefix(root)
            .expect("validated config entrypoint")
            .to_path_buf();
        let in_roots = roots.values().any(|scan_root| {
            scan_root.path.as_os_str().is_empty()
                || if scan_root.is_file {
                    relative == scan_root.path
                } else {
                    relative == scan_root.path || relative.starts_with(&scan_root.path)
                }
        });
        let display = relative.to_string_lossy().replace('\\', "/");
        if !in_roots
            || exclude
                .matched_path_or_any_parents(&canonical, false)
                .is_ignore()
            || is_test_path(&display)
            || is_declaration_file(&display)
        {
            return Err(format!(
                "invalid {CONFIG_PATH} entrypoint {value:?}: entrypoint must be an included runtime source file"
            ));
        }
        entrypoints.insert(display);
    }
    Ok(ScanConfig {
        roots: roots.into_values().collect(),
        entrypoints,
        exclude,
        exclude_patterns,
    })
}

fn discover_entrypoints(
    project: &Project,
    included_paths: &BTreeSet<String>,
) -> Result<EntrypointDiscovery, String> {
    let mut files = project.config.entrypoints.clone();
    if let Some(path) = files.iter().find(|path| !included_paths.contains(*path)) {
        return Err(format!(
            "invalid {CONFIG_PATH} entrypoint {path:?}: file is outside the collected source graph"
        ));
    }
    let mut public_files = BTreeSet::new();
    let explicit = !files.is_empty();
    let mut diagnostics = Vec::new();
    let mut framework_incomplete = false;

    if !explicit {
        let html_path = project.root.join("index.html");
        if html_path.is_file() {
            let html = fs::read_to_string(&html_path)
                .map_err(|error| format!("cannot read index.html: {error}"))?;
            for source in html_module_sources(&html) {
                if let Some(path) = resolve_entrypoint(project, included_paths, source) {
                    files.insert(path);
                }
            }
        }

        let package_path = project.root.join("package.json");
        if package_path.is_file() {
            let source = fs::read_to_string(&package_path)
                .map_err(|error| format!("cannot read package.json: {error}"))?;
            match serde_json::from_str::<Value>(&source) {
                Ok(package) => {
                    for path in package_runtime_entrypoint_values(&package) {
                        if let Some(path) = resolve_entrypoint(project, included_paths, path) {
                            files.insert(path);
                        }
                    }
                    for path in package_public_entrypoint_values(&package) {
                        if let Some(path) = resolve_entrypoint(project, included_paths, path) {
                            public_files.insert(path);
                        }
                    }
                    if let Some(scripts) = package.get("scripts").and_then(Value::as_object) {
                        for (name, value) in scripts {
                            let Some(command) = value.as_str() else {
                                continue;
                            };
                            if let Some(path) = script_entrypoint(command)
                                .and_then(|path| resolve_entrypoint(project, included_paths, path))
                            {
                                files.insert(path);
                            } else if matches!(name.as_str(), "start" | "serve") {
                                diagnostics.push(Diagnostic {
                                    code: "entrypoint-script-unsupported".to_owned(),
                                    path: Some("package.json".to_owned()),
                                    line: None,
                                    message: format!(
                                        "cannot statically resolve package script {name:?}; configure entrypoints in {CONFIG_PATH} version {CONFIG_VERSION}"
                                    ),
                                });
                            }
                        }
                    }
                    if package_uses_framework(&package, "nuxt")
                        || package_uses_framework(&package, "next")
                    {
                        framework_incomplete = true;
                        diagnostics.push(Diagnostic {
                            code: "framework-semantic-incomplete".to_owned(),
                            path: Some("package.json".to_owned()),
                            line: None,
                            message: "Nuxt/Next convention and auto-import analysis is deferred; affected global results are unknown"
                                .to_owned(),
                        });
                    }
                }
                Err(error) => {
                    diagnostics.push(Diagnostic {
                        code: "entrypoint-package-json".to_owned(),
                        path: Some("package.json".to_owned()),
                        line: None,
                        message: error.to_string(),
                    });
                }
            }
        }
    }

    if explicit {
        let package_path = project.root.join("package.json");
        if package_path.is_file() {
            let source = fs::read_to_string(&package_path)
                .map_err(|error| format!("cannot read package.json: {error}"))?;
            if let Ok(package) = serde_json::from_str::<Value>(&source) {
                for path in package_public_entrypoint_values(&package) {
                    if let Some(path) = resolve_entrypoint(project, included_paths, path) {
                        public_files.insert(path);
                    }
                }
                if package_uses_framework(&package, "nuxt")
                    || package_uses_framework(&package, "next")
                {
                    framework_incomplete = true;
                    diagnostics.push(Diagnostic {
                        code: "framework-semantic-incomplete".to_owned(),
                        path: Some("package.json".to_owned()),
                        line: None,
                        message: "Nuxt/Next convention and auto-import analysis is deferred; affected global results are unknown"
                            .to_owned(),
                    });
                }
            }
        }
    }

    if !files.is_empty() {
        diagnostics.retain(|diagnostic| diagnostic.code != "entrypoint-script-unsupported");
    }
    let complete = !files.is_empty();
    if files.is_empty() {
        diagnostics.push(Diagnostic {
            code: "entrypoint-coverage-incomplete".to_owned(),
            path: None,
            line: None,
            message: format!(
                "cannot prove a project entrypoint; configure entrypoints in {CONFIG_PATH} version {CONFIG_VERSION}"
            ),
        });
    }

    Ok(EntrypointDiscovery {
        files,
        public_files,
        complete,
        framework_incomplete,
        diagnostics,
    })
}

fn html_module_sources(html: &str) -> Vec<&str> {
    let mut sources = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script") {
        rest = &rest[start + "<script".len()..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if html_attribute(tag, "type") != Some("module") {
            continue;
        }
        if let Some(source) = html_attribute(tag, "src") {
            sources.push(source);
        }
    }
    sources
}

fn html_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    for quote in ['"', '\''] {
        let prefix = format!("{name}={quote}");
        let Some(start) = tag.find(&prefix).map(|start| start + prefix.len()) else {
            continue;
        };
        let value = &tag[start..];
        if let Some(end) = value.find(quote) {
            return Some(&value[..end]);
        }
    }
    None
}

fn package_runtime_entrypoint_values(package: &Value) -> Vec<&str> {
    package_values(package, &["bin"])
}

fn package_public_entrypoint_values(package: &Value) -> Vec<&str> {
    package_values(package, &["bin", "main", "module", "exports"])
}

fn package_values<'a>(package: &'a Value, keys: &[&str]) -> Vec<&'a str> {
    fn collect<'a>(value: &'a Value, output: &mut Vec<&'a str>) {
        match value {
            Value::String(value) if !value.contains('*') => output.push(value),
            Value::Array(values) => {
                for value in values {
                    collect(value, output);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    collect(value, output);
                }
            }
            _ => {}
        }
    }

    let mut output = Vec::new();
    for key in keys {
        if let Some(value) = package.get(*key) {
            collect(value, &mut output);
        }
    }
    output
}

fn script_entrypoint(command: &str) -> Option<&str> {
    if command
        .chars()
        .any(|character| matches!(character, '|' | '&' | ';' | '$' | '`'))
    {
        return None;
    }
    let mut words = command.split_whitespace();
    let launcher = Path::new(words.next()?)
        .file_name()
        .and_then(|name| name.to_str())?;
    if !matches!(
        launcher,
        "node" | "bun" | "deno" | "tsx" | "ts-node" | "vite-node"
    ) {
        return None;
    }
    if launcher == "deno" && words.next()? != "run" {
        return None;
    }
    words.find(|word| !word.starts_with('-'))
}

fn package_uses_framework(package: &Value, dependency: &str) -> bool {
    ["dependencies", "devDependencies"]
        .into_iter()
        .filter_map(|key| package.get(key).and_then(Value::as_object))
        .any(|dependencies| dependencies.contains_key(dependency))
}

fn resolve_entrypoint(
    project: &Project,
    included_paths: &BTreeSet<String>,
    value: &str,
) -> Option<String> {
    let value = value
        .split(['?', '#'])
        .next()?
        .trim_start_matches("./")
        .trim_start_matches('/');
    let relative = config_relative_path(value, "entrypoint").ok()?;
    let mut candidates = vec![relative.clone()];
    if relative.extension().is_none() {
        for extension in ["ts", "tsx", "js", "jsx", "mjs", "cjs", "vue"] {
            candidates.push(relative.with_extension(extension));
        }
    }
    candidates.into_iter().find_map(|relative| {
        let path = project.root.join(&relative);
        let display = relative.to_string_lossy().replace('\\', "/");
        (path.is_file()
            && is_source(&path)
            && included_paths.contains(&display)
            && project.config.includes(&project.root, &relative, false)
            && !is_test_path(&relative.to_string_lossy())
            && !is_declaration_file(&relative.to_string_lossy()))
        .then_some(display)
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
    let mut consumer_paths = BTreeSet::new();
    let mut runtime_consumer_roots = BTreeSet::new();
    let mut type_consumer_roots = BTreeSet::new();
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
        let relative_path = Path::new(&relative);
        if !project.config.contains(relative_path) || is_test_path(&relative) {
            continue;
        }
        let excluded = project.config.is_excluded(root, relative_path, false);
        let declaration = is_declaration_file(&relative);
        match fs::read_to_string(&path) {
            Ok(content) => {
                consumer_paths.insert(relative.clone());
                if declaration {
                    type_consumer_roots.insert(relative.clone());
                } else {
                    if excluded {
                        runtime_consumer_roots.insert(relative.clone());
                    } else {
                        included_paths.insert(relative.clone());
                    }
                }
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
                .with_candidate_collection(included_paths.contains(path.as_str()))
        })
        .collect::<Vec<_>>();
    blocks.extend(vue_files.iter().map(|path| {
        SourceBlock::new(path.clone(), "", 0, "ts")
            .with_candidate_collection(included_paths.contains(path))
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
                    let collect_candidates = included_paths.contains(&script.path);
                    script.blocks.into_iter().map(move |block| {
                        SourceBlock::new(
                            script.path.clone(),
                            block.content,
                            block.offset,
                            block.lang,
                        )
                        .with_candidate_collection(collect_candidates)
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
        consumer_paths,
        runtime_consumer_roots,
        type_consumer_roots,
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
        (
            &left.path,
            left.line,
            left.column,
            left.kind,
            &left.name,
            &left.id,
        )
            .cmp(&(
                &right.path,
                right.line,
                right.column,
                right.kind,
                &right.name,
                &right.id,
            ))
    });
    let sort_classified = |left: &Classified, right: &Classified| {
        (
            &left.path,
            left.line,
            left.column,
            left.kind,
            &left.name,
            &left.id,
        )
            .cmp(&(
                &right.path,
                right.line,
                right.column,
                right.kind,
                &right.name,
                &right.id,
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
    fn structural_filters_cover_declarations_and_tests_without_guessing_entrypoints() {
        assert_eq!(structural_ignore("src/main.ts"), None);
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

    #[test]
    fn entrypoint_parsers_accept_only_static_sources() {
        assert_eq!(
            html_module_sources(
                r#"<script src="./legacy.js"></script><script type='module' src='/src/main.ts'></script>"#
            ),
            ["/src/main.ts"]
        );
        assert_eq!(
            script_entrypoint("node src/server.js"),
            Some("src/server.js")
        );
        assert_eq!(
            script_entrypoint("deno run --allow-net src/server.ts"),
            Some("src/server.ts")
        );
        assert_eq!(script_entrypoint("NODE_ENV=prod node src/server.js"), None);
        assert_eq!(script_entrypoint("node src/a.js | tee output"), None);
        let package = serde_json::json!({
            "bin": "src/cli.ts",
            "main": "src/sdk.ts",
            "module": "src/sdk.mts",
            "exports": "src/index.ts",
        });
        assert_eq!(package_runtime_entrypoint_values(&package), ["src/cli.ts"]);
        assert_eq!(
            package_public_entrypoint_values(&package),
            ["src/cli.ts", "src/sdk.ts", "src/sdk.mts", "src/index.ts"]
        );
    }

    #[test]
    fn golden_fixture_reachability_follows_entrypoint_imports() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unused-golden");
        let evidence = build_evidence(&root, true).expect("golden evidence");
        let reachability = policy::compute(
            &evidence.scan,
            &evidence.semantic_edges,
            &evidence.entrypoints,
            evidence.entrypoints_complete,
            &evidence.sources.runtime_consumer_roots,
            &evidence.sources.type_consumer_roots,
        );
        let used = evidence
            .scan
            .candidates
            .iter()
            .find(|candidate| candidate.name == "usedDirectly")
            .expect("usedDirectly");
        assert!(
            reachability.used_candidates.contains(&used.id),
            "{reachability:#?}"
        );
    }
}
