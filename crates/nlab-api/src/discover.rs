use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;
use regex::Regex;
use serde::Serialize;

use crate::config::{
    DiscoveredService, LocalProjectConfig, ProjectConfig, ensure_local_config_ignored,
};
use crate::graph::Snapshot;
use crate::java::{JavaProject, parse_method_signature};
use crate::semantic::{RemoteCall, SemanticAnalyzer};
use crate::{absolute_path, repo};

mod acquisition;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Args)]
pub struct DiscoverArgs {
    /// Frontend project containing the existing generation config
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// Directory containing backend Git repositories and their CodeGraph indexes
    #[arg(long)]
    repositories_root: Option<PathBuf>,
    /// Explicitly permit a currently missing service; repeat for multiple services
    #[arg(long, value_name = "service")]
    allow_missing: Vec<String>,
    /// Interface file relative to the configured backend; defaults to contractRoots
    #[arg(long)]
    entry: Option<PathBuf>,
    /// Skip SIC queries and cloning; still synchronize local repository indexes
    #[arg(long)]
    offline: bool,
    /// Maximum cross-repository hops (local calls do not consume this budget)
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=16))]
    max_depth: u8,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    version: u8,
    project: PathBuf,
    repositories_root: PathBuf,
    configured_branch: String,
    entries: Vec<String>,
    repositories: Vec<RepositoryInfo>,
    calls: Vec<CallResolution>,
    visited_methods: usize,
    unresolved_local_calls: usize,
    warnings: Vec<String>,
    blocking_services: Vec<String>,
    pub allowed_missing_services: Vec<String>,
    acquisitions: BTreeMap<String, acquisition::Acquisition>,
}

pub(crate) struct DiscoveryRun {
    pub report: Report,
    pub graph: Snapshot,
}

#[derive(Debug)]
pub(crate) struct Blocked(pub Vec<String>);

impl std::fmt::Display for Blocked {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            output,
            "discovery blocked: {}; add the missing repositories/indexes, or explicitly record `discover --allow-missing <service>` for missing repositories",
            self.0.join(", ")
        )
    }
}
impl std::error::Error for Blocked {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryInfo {
    path: PathBuf,
    repository: Option<String>,
    branch: Option<String>,
    commit: Option<String>,
    dirty: Option<bool>,
    services: BTreeSet<String>,
    index: Option<PathBuf>,
    index_status: String,
    codegraph_version: Option<String>,
    extraction_version: Option<String>,
    index_freshness: &'static str,
    visited: bool,
    diagnostics: Vec<String>,
}

struct Repository {
    info: RepositoryInfo,
    graph: Option<Snapshot>,
    references: BTreeMap<String, Vec<Binding>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Binding {
    service: String,
    file: String,
    line: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CallResolution {
    from_repository: PathBuf,
    depth: u8,
    #[serde(flatten)]
    call: RemoteCall,
    bindings: Vec<Binding>,
    status: String,
    candidates: Vec<PathBuf>,
    search_terms: Vec<String>,
}

pub fn run(args: DiscoverArgs) -> u8 {
    let result = (|| {
        let project = args.project.canonicalize()?;
        ProjectConfig::load(&project)?;
        let _lock = crate::output::OutputLock::acquire(&project)?;
        let mut result = acquisition::scan(
            args.clone(),
            Instant::now() + Duration::from_secs(crate::MAX_TIMEOUT_SECONDS),
        )?;
        record(
            &project,
            &mut result.report,
            &args.allow_missing,
            args.entry.is_some(),
        )?;
        save_root(&project, &result.report.repositories_root)?;
        Ok::<_, anyhow::Error>(result.report)
    })();
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("serialize discovery")
            );
            if report.blocking_services.is_empty() {
                0
            } else {
                2
            }
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

#[cfg(test)]
fn discover(args: DiscoverArgs) -> Result<Report> {
    Ok(scan(args, &BTreeMap::new())?.report)
}

pub(crate) fn save_root(project: &Path, root: &Path) -> Result<()> {
    if !root.is_dir() {
        bail!("repositories root must be a directory: {}", root.display());
    }
    let mut local = LocalProjectConfig::load(project)?;
    local.backend.repositories_root =
        Some(root.canonicalize().context("resolve repositories root")?);
    ensure_local_config_ignored(project)?;
    local.save(project)
}

pub(crate) fn prepare(
    project: &Path,
    root: &Path,
    offline: bool,
    deadline: Instant,
) -> Result<DiscoveryRun> {
    let result = acquisition::scan(
        DiscoverArgs {
            project: project.to_owned(),
            repositories_root: Some(root.to_owned()),
            entry: None,
            max_depth: 3,
            allow_missing: Vec::new(),
            offline,
        },
        deadline,
    )?;
    finish_preflight(project, result)
}

fn finish_preflight(project: &Path, mut result: DiscoveryRun) -> Result<DiscoveryRun> {
    record(project, &mut result.report, &[], false)?;
    if !result.report.blocking_services.is_empty() {
        crate::output::write_report(
            project,
            &serde_json::json!({
                "version": 1, "status": "blocked", "stages": { "discovery": result.report, "generate": "not-started" }
            }),
        )?;
        return Err(Blocked(result.report.blocking_services.clone()).into());
    }
    Ok(result)
}

fn record(project: &Path, report: &mut Report, allowed: &[String], partial: bool) -> Result<()> {
    let mut state = ProjectConfig::load(project)?.discovery.unwrap_or_default();
    let previous = state.services.clone();
    if !partial {
        for service in state.services.values_mut() {
            service.status = "unused".to_owned();
        }
    }
    let mut observed = BTreeMap::<String, DiscoveredService>::new();
    for call in &report.calls {
        // Library interfaces without an RPC binding are evidence, not invented services.
        let names = if call.bindings.is_empty() {
            vec![format!("interface:{}", call.call.interface)]
        } else {
            call.bindings
                .iter()
                .map(|binding| binding.service.clone())
                .collect()
        };
        let repository = call
            .candidates
            .first()
            .filter(|_| call.candidates.len() == 1)
            .and_then(|path| {
                report
                    .repositories
                    .iter()
                    .find(|repository| repository.path == *path)
            })
            .and_then(|repository| repository.repository.clone());
        let status = match call.status.as_str() {
            "source-matched" => "resolved",
            "missing-source" => "missing",
            other => other,
        };
        for name in names {
            let service = observed
                .entry(name.clone())
                .or_insert_with(|| DiscoveredService {
                    repository: previous
                        .get(&name)
                        .and_then(|service| service.repository.clone()),
                    allow_missing: previous
                        .get(&name)
                        .is_some_and(|service| service.allow_missing),
                    status: "resolved".to_owned(),
                    ..DiscoveredService::default()
                });
            service.interfaces.insert(call.call.interface.clone());
            if repository.is_some() {
                service.repository = repository.clone();
            }
            let priority = |status: &str| match status {
                "missing" => 6,
                "index-unavailable" => 5,
                "ambiguous" => 4,
                "depth-limit" => 3,
                "method-unresolved" => 2,
                "unbound-candidate" => 1,
                _ => 0,
            };
            if priority(status) > priority(&service.status) {
                service.status = status.to_owned();
            }
            if !call.candidates.is_empty() {
                service.allow_missing = false;
            }
        }
    }
    for (name, acquisition) in &report.acquisitions {
        if let Some(service) = observed.get_mut(name) {
            if acquisition.repository.is_some() {
                service.repository = acquisition.repository.clone();
            }
            if matches!(service.status.as_str(), "missing" | "acquisition-failed")
                && acquisition.status == "acquisition-failed"
            {
                service.status = "acquisition-failed".to_owned();
                service.error = acquisition.error.clone();
            }
        }
    }
    for name in allowed {
        let service = observed
            .get_mut(name)
            .with_context(|| format!("service not found in current discovery: {name}"))?;
        if !matches!(service.status.as_str(), "missing" | "acquisition-failed") {
            bail!("service {name} is not missing; repair its association/index instead");
        }
        service.allow_missing = true;
    }
    for (name, service) in &observed {
        if matches!(service.status.as_str(), "missing" | "acquisition-failed")
            && service.allow_missing
        {
            report.allowed_missing_services.push(name.clone());
        }
        if (matches!(service.status.as_str(), "missing" | "acquisition-failed")
            && !service.allow_missing)
            || (!name.starts_with("interface:")
                && matches!(
                    service.status.as_str(),
                    "index-unavailable" | "ambiguous" | "unbound-candidate"
                ))
        {
            report.blocking_services.push(name.clone());
        }
    }
    state.services.extend(observed);
    ProjectConfig::save_discovery(project, &state)
}

fn scan(
    args: DiscoverArgs,
    synchronized: &BTreeMap<PathBuf, Result<(), String>>,
) -> Result<DiscoveryRun> {
    let project_path = absolute_path(&args.project)?.canonicalize()?;
    let config = ProjectConfig::load(&project_path)?;
    let root = args
        .repositories_root
        .or(LocalProjectConfig::load(&project_path)?
            .backend
            .repositories_root)
        .context("repositories root missing; pass --repositories-root <path>")?;
    let root = absolute_path(&root)?.canonicalize()?;
    let backend = repo::resolve_path(
        &config.backend.repo_path,
        config.backend.repository.as_deref(),
    )?;
    if !backend.is_dir() {
        bail!(
            "configured backend is not cloned: {}; repository: {}",
            backend.display(),
            config.backend.repository.as_deref().unwrap_or("unknown")
        );
    }
    let backend = backend.canonicalize()?;
    let mut paths = fs::read_dir(&root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.join(".git").exists());
    let mut paths = paths
        .into_iter()
        .map(|path| path.canonicalize())
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.push(backend.clone());
    paths.sort();
    paths.dedup();
    let mut repositories = paths
        .iter()
        .map(|path| load_repository(path, synchronized.get(path)))
        .collect::<Result<Vec<_>>>()?;
    let initial = repositories
        .iter()
        .position(|repository| repository.info.path == backend)
        .unwrap();
    let graph = repositories[initial].graph.as_ref().with_context(|| {
        format!(
            "entry repository has no usable CodeGraph index: {:?}",
            repositories[initial].info.diagnostics
        )
    })?;
    let entry = args
        .entry
        .map(|entry| {
            let entry = backend
                .join(entry)
                .canonicalize()
                .context("resolve interface entry")?;
            if !entry.starts_with(&backend) {
                bail!("interface entry must be inside configured backend");
            }
            Ok(entry.strip_prefix(&backend)?.to_path_buf())
        })
        .transpose()?;
    let mut entry_ids = Vec::new();
    let mut entries = Vec::new();
    for node in graph.nodes.values().filter(|node| node.kind == "interface") {
        let selected = match &entry {
            Some(entry) => Path::new(&node.file_path) == entry,
            None => config
                .backend
                .contract_roots
                .iter()
                .any(|root| Path::new(&node.file_path).starts_with(root)),
        };
        if selected {
            entries.push(node.qualified_name.replace("::", "."));
            entry_ids.extend(
                graph
                    .contained(&node.id, "method")
                    .iter()
                    .map(|method| method.id.clone()),
            );
        }
    }
    if entry_ids.is_empty() {
        bail!("no indexed interface methods found in selected entry/contractRoots");
    }
    entry_ids.sort();
    entries.sort();
    let mut report = Report {
        version: 1, project: project_path, repositories_root: root,
        configured_branch: config.backend.branch.clone(), entries, repositories: Vec::new(),
        calls: Vec::new(), visited_methods: 0, unresolved_local_calls: 0,
        blocking_services: Vec::new(),
        allowed_missing_services: Vec::new(),
        acquisitions: BTreeMap::new(),
        warnings: vec!["Discovery reports local source candidates, not deployed RPC bindings. Each repository uses its own CodeGraph index.".to_owned()],
    };
    if repositories[initial].info.branch.as_deref() != Some(&config.backend.branch) {
        report.warnings.push(format!(
            "configured branch is {}; inspecting current checkout {} without switching",
            config.backend.branch,
            repositories[initial]
                .info
                .branch
                .as_deref()
                .unwrap_or("detached/unknown")
        ));
    }
    let mut catalog = BTreeMap::<String, BTreeSet<usize>>::new();
    for (index, repository) in repositories.iter().enumerate() {
        if let Some(graph) = &repository.graph {
            for node in graph.nodes.values().filter(|node| node.kind == "interface") {
                catalog
                    .entry(node.qualified_name.replace("::", "."))
                    .or_default()
                    .insert(index);
            }
        }
    }
    let mut queue = VecDeque::from([(initial, entry_ids, 0)]);
    let mut scheduled = BTreeSet::new();
    let mut visited = BTreeMap::<usize, BTreeSet<String>>::new();
    while let Some((index, roots, depth)) = queue.pop_front() {
        let roots = roots
            .into_iter()
            .filter(|id| scheduled.insert((index, id.clone())))
            .collect::<Vec<_>>();
        if roots.is_empty() {
            continue;
        }
        repositories[index].info.visited = true;
        let repository = &repositories[index];
        let graph = repository
            .graph
            .as_ref()
            .context("scheduled repository has no index")?;
        let project = match JavaProject::load(&repository.info.path, graph) {
            Ok(project) => project,
            Err(error) => {
                report
                    .warnings
                    .push(format!("{}: {error:#}", repository.info.path.display()));
                continue;
            }
        };
        let remote_types = repository
            .references
            .keys()
            .cloned()
            .chain(
                catalog
                    .iter()
                    .filter(|(_, owners)| !owners.contains(&index))
                    .map(|(name, _)| name.clone()),
            )
            .collect();
        let walk = SemanticAnalyzer::new(&project).discover_calls(
            &roots,
            &remote_types,
            visited.entry(index).or_default(),
        )?;
        report.visited_methods += walk.visited_methods;
        report.unresolved_local_calls += walk.unresolved_calls;
        report.warnings.extend(
            walk.gaps
                .into_iter()
                .map(|gap| format!("{}: {gap}", repository.info.path.display())),
        );
        for call in walk.calls {
            let bindings = repository
                .references
                .get(&call.interface)
                .cloned()
                .unwrap_or_default();
            let services = bindings
                .iter()
                .map(|binding| binding.service.as_str())
                .collect::<BTreeSet<_>>();
            let service_candidates = repositories
                .iter()
                .enumerate()
                .filter(|(candidate, repository)| {
                    *candidate != index
                        && services
                            .iter()
                            .any(|service| repository.info.services.contains(*service))
                })
                .map(|(candidate, _)| candidate)
                .collect::<Vec<_>>();
            let bound = !service_candidates.is_empty();
            let candidates = if bound {
                service_candidates
            } else {
                catalog
                    .get(&call.interface)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|candidate| *candidate != index)
                    .collect()
            };
            let mut search_terms = vec![call.interface.clone()];
            search_terms.extend(services.iter().map(|service| (*service).to_owned()));
            let status = if candidates.is_empty() {
                "missing-source"
            } else if candidates.len() != 1 || services.len() > 1 {
                "ambiguous"
            } else if !bound {
                "unbound-candidate"
            } else if repositories[candidates[0]].graph.is_none() {
                "index-unavailable"
            } else {
                let target = candidates[0];
                let methods = target_methods(repositories[target].graph.as_ref().unwrap(), &call);
                if methods.len() != 1 {
                    "method-unresolved"
                } else if depth >= args.max_depth {
                    "depth-limit"
                } else {
                    queue.push_back((target, methods, depth + 1));
                    "source-matched"
                }
            };
            report.calls.push(CallResolution {
                from_repository: repository.info.path.clone(),
                depth,
                call,
                bindings,
                status: status.to_owned(),
                candidates: candidates
                    .iter()
                    .map(|index| repositories[*index].info.path.clone())
                    .collect(),
                search_terms,
            });
        }
    }
    report.calls.sort_by(|a, b| {
        (
            &a.from_repository,
            &a.call.file,
            a.call.line,
            a.call.column,
            &a.call.method,
        )
            .cmp(&(
                &b.from_repository,
                &b.call.file,
                b.call.line,
                b.call.column,
                &b.call.method,
            ))
    });
    report.calls.dedup_by(|a, b| {
        a.from_repository == b.from_repository
            && a.call.file == b.call.file
            && a.call.line == b.call.line
            && a.call.column == b.call.column
            && a.call.interface == b.call.interface
            && a.call.method == b.call.method
    });
    report.warnings.sort();
    report.warnings.dedup();
    let mut graph = repositories[initial]
        .graph
        .take()
        .context("entry graph missing")?;
    for (index, repository) in repositories.iter_mut().enumerate() {
        if index != initial && repository.info.visited {
            if let Some(dependency) = repository.graph.take() {
                graph.include_repository(&repository.info.path, dependency)?;
            }
        }
    }
    for call in report
        .calls
        .iter()
        .filter(|call| call.status == "source-matched")
    {
        let file = if call.from_repository == backend {
            call.call.file.clone()
        } else {
            format!(
                "__dependencies/{}/{}",
                call.from_repository.file_name().unwrap().to_string_lossy(),
                call.call.file
            )
        };
        graph.resolved_external_calls.insert((
            file,
            call.call.line,
            call.call.column,
            call.call.interface.clone(),
        ));
    }
    report.repositories = repositories
        .into_iter()
        .map(|repository| repository.info)
        .collect();
    Ok(DiscoveryRun { report, graph })
}

fn target_methods(graph: &Snapshot, call: &RemoteCall) -> Vec<String> {
    graph
        .nodes
        .values()
        .filter(|node| {
            node.kind == "interface" && node.qualified_name.replace("::", ".") == call.interface
        })
        .flat_map(|node| graph.contained(&node.id, "method"))
        .filter(|method| {
            method.name == call.method
                && call.arity.is_none_or(|arity| {
                    parse_method_signature(&method.signature)
                        .is_some_and(|(_, parameters)| parameters.len() == arity)
                })
        })
        .map(|method| method.id.clone())
        .collect()
}

fn load_repository(path: &Path, synchronized: Option<&Result<(), String>>) -> Result<Repository> {
    let path = path.canonicalize()?;
    let mut diagnostics = Vec::new();
    let mut git = |args: &[&str]| -> Option<String> {
        match repo::git_text(&path, args) {
            Ok(value) => Some(value),
            Err(error) => {
                diagnostics.push(format!("{error:#}"));
                None
            }
        }
    };
    let origin = git(&["remote", "get-url", "origin"]).map(|origin| public_origin(&origin));
    let branch = git(&["branch", "--show-current"]);
    let commit = git(&["rev-parse", "HEAD"]);
    let dirty = git(&["status", "--porcelain"]).map(|status| !status.is_empty());
    let index = path.join(".codegraph/codegraph.db");
    let graph = match synchronized {
        Some(Err(error)) => {
            diagnostics.push(error.clone());
            None
        }
        _ if index.is_file() => match Snapshot::load(&path) {
            Ok(snapshot) if !snapshot.nodes.is_empty() => Some(snapshot),
            Ok(_) => {
                diagnostics.push("repository has no nodes in index".to_owned());
                None
            }
            Err(error) => {
                diagnostics.push(format!("{error:#}"));
                None
            }
        },
        _ => None,
    };
    let mut services = BTreeSet::new();
    let mut references = BTreeMap::<String, Vec<Binding>>::new();
    for item in ignore::WalkBuilder::new(&path)
        .hidden(true)
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some("target" | "build" | ".build" | "node_modules")
            )
        })
        .build()
    {
        let entry =
            item.with_context(|| format!("scan service configuration in {}", path.display()))?;
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "xml")
        {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&path)?
            .to_string_lossy()
            .into_owned();
        if !relative.contains("/src/main/resources/")
            && !relative.starts_with("src/main/resources/")
        {
            continue;
        }
        let source = fs::read_to_string(entry.path())?;
        let parsed = scf_bindings(&source, &relative)?;
        services.extend(parsed.0);
        for (interface, bindings) in parsed.1 {
            references.entry(interface).or_default().extend(bindings);
        }
    }
    for bindings in references.values_mut() {
        bindings.sort_by(|a, b| (&a.service, &a.file, a.line).cmp(&(&b.service, &b.file, b.line)));
    }
    let info = RepositoryInfo {
        path: path.clone(),
        repository: origin,
        branch,
        commit,
        dirty,
        services,
        index: index.is_file().then_some(index.clone()),
        index_status: if graph.is_some() {
            "available"
        } else if index.is_file() {
            "unusable"
        } else {
            "missing"
        }
        .to_owned(),
        index_freshness: if synchronized.is_some_and(Result::is_ok) {
            "synchronized-before-discovery"
        } else {
            "not-verified"
        },
        codegraph_version: graph.as_ref().map(|graph| graph.version.clone()),
        extraction_version: graph.as_ref().map(|graph| graph.extraction_version.clone()),
        visited: false,
        diagnostics,
    };
    Ok(Repository {
        info,
        graph,
        references,
    })
}

fn public_origin(origin: &str) -> String {
    let origin = origin.split(['?', '#']).next().unwrap_or(origin);
    if let Some((scheme, rest)) = origin.split_once("://") {
        return format!(
            "{scheme}://{}",
            rest.split_once('@').map_or(rest, |(_, host)| host)
        );
    }
    origin.to_owned()
}

type Bindings = (BTreeSet<String>, BTreeMap<String, Vec<Binding>>);

/// Recognize literal SCF bindings only; never evaluate Spring placeholders or load XML entities.
fn scf_bindings(source: &str, file: &str) -> Result<Bindings> {
    let comments = Regex::new(r"(?s)<!--.*?-->")?;
    let source = comments.replace_all(source, |captures: &regex::Captures<'_>| {
        captures[0]
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect::<String>()
    });
    let tags = Regex::new(r"(?s)<(/?)(?:[\w-]+:)?(references|reference|application)\b([^>]*)>")?;
    let attributes = Regex::new(r#"([\w-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)?;
    let mut services = BTreeSet::new();
    let mut references = BTreeMap::<String, Vec<Binding>>::new();
    let mut service = None;
    for tag in tags.captures_iter(&source) {
        let values = attributes
            .captures_iter(&tag[3])
            .map(|attribute| {
                (
                    attribute[1].to_owned(),
                    attribute
                        .get(2)
                        .or_else(|| attribute.get(3))
                        .unwrap()
                        .as_str()
                        .to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        match &tag[2] {
            "application" => {
                if let Some(name) = values.get("applicationName") {
                    services.insert(name.clone());
                }
            }
            "references" => {
                service = if &tag[1] == "/" || tag[3].trim_end().ends_with('/') {
                    None
                } else {
                    values.get("serviceName").cloned()
                };
            }
            "reference" if &tag[1] != "/" => {
                if let (Some(interface), Some(service)) = (
                    values.get("interface"),
                    values.get("serviceName").or(service.as_ref()),
                ) {
                    references
                        .entry(interface.clone())
                        .or_default()
                        .push(Binding {
                            service: service.clone(),
                            file: file.to_owned(),
                            line: source[..tag.get(0).unwrap().start()]
                                .bytes()
                                .filter(|byte| *byte == b'\n')
                                .count()
                                + 1,
                        });
                }
            }
            _ => {}
        }
    }
    Ok((services, references))
}
