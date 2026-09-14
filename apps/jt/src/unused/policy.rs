use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use super::oxc::{GraphEdge, ScanResult};
use super::sidecar::SemanticEdge;

/// Results of static, owner-aware reachability.
///
/// `used_candidates` contains declarations reached by runtime or type
/// evidence. `local_used_candidates` is the subset with same-file evidence.
/// `used_files`
/// contains files reached by runtime or type module evidence.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Reachability {
    pub(crate) used_candidates: BTreeSet<String>,
    pub(crate) used_files: BTreeSet<String>,
    pub(crate) runtime_used_files: BTreeSet<String>,
    pub(crate) local_used_candidates: BTreeSet<String>,
    pub(crate) active_runtime_owners: BTreeSet<String>,
}

/// Compute deterministic reachability without treating declarations or
/// re-exports as roots. Runtime owner activation is required before calls and
/// references can protect another declaration.
pub(crate) fn compute(
    scan: &ScanResult,
    semantic_edges: &[SemanticEdge],
    entrypoints: &BTreeSet<String>,
    entrypoints_complete: bool,
    runtime_consumer_roots: &BTreeSet<String>,
    type_consumer_roots: &BTreeSet<String>,
) -> Reachability {
    let mut graph = ReachabilityGraph::new(scan, semantic_edges);

    for root in entrypoints {
        graph.seed_runtime_root(root, false);
    }
    for root in runtime_consumer_roots {
        graph.seed_runtime_root(root, true);
    }
    for root in type_consumer_roots {
        graph.seed_type_root(root);
    }

    graph.run();

    // When entrypoint discovery is incomplete, local non-exported evidence is
    // still safe: it does not claim that file or exported API has a consumer.
    if !entrypoints_complete {
        let files = scan
            .candidates
            .iter()
            .filter(|candidate| !candidate.exported)
            .map(|candidate| candidate.path.clone())
            .collect::<BTreeSet<_>>();
        for file in files {
            let mut local = ReachabilityGraph::new(scan, semantic_edges);
            local.seed_runtime_root(&file, false);
            local.run();
            for candidate in scan
                .candidates
                .iter()
                .filter(|candidate| !candidate.exported && candidate.path == file)
            {
                if local.result.used_candidates.contains(&candidate.id) {
                    graph
                        .result
                        .local_used_candidates
                        .insert(candidate.id.clone());
                    graph.result.used_candidates.insert(candidate.id.clone());
                }
            }
        }
    }

    graph.result.active_runtime_owners = graph.active_runtime_owners.clone();
    graph.result
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum EdgeMode {
    Runtime,
    Type,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Edge {
    source: String,
    target: String,
    kind: String,
    mode: EdgeMode,
}

struct ReachabilityGraph<'a> {
    scan: &'a ScanResult,
    candidates: HashMap<String, usize>,
    owners: HashMap<String, usize>,
    owners_by_parent: HashMap<String, Vec<String>>,
    constructors_by_class: HashMap<String, Vec<String>>,
    edges_by_source: HashMap<String, Vec<Edge>>,
    runtime_queue: VecDeque<String>,
    type_queue: VecDeque<String>,
    active_runtime_owners: BTreeSet<String>,
    active_type_owners: BTreeSet<String>,
    result: Reachability,
}

impl<'a> ReachabilityGraph<'a> {
    fn new(scan: &'a ScanResult, semantic_edges: &[SemanticEdge]) -> Self {
        let candidates = scan
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| (candidate.id.clone(), index))
            .collect::<HashMap<_, _>>();
        let owners = scan
            .execution_owners
            .iter()
            .enumerate()
            .map(|(index, owner)| (owner.id.clone(), index))
            .collect::<HashMap<_, _>>();
        let mut owners_by_parent = HashMap::<String, Vec<String>>::new();
        for owner in &scan.execution_owners {
            if let Some(parent) = owner.parent_id.as_ref() {
                owners_by_parent
                    .entry(parent.clone())
                    .or_default()
                    .push(owner.id.clone());
            }
        }
        for children in owners_by_parent.values_mut() {
            children.sort();
        }

        let mut constructors_by_class = HashMap::<String, Vec<String>>::new();
        for owner in &scan.execution_owners {
            if owner.kind != "constructor" {
                continue;
            }
            let Some(constructor_id) = owner.finding_candidate_id.as_deref() else {
                continue;
            };
            let Some(constructor) = scan
                .candidates
                .iter()
                .find(|candidate| candidate.id == constructor_id)
            else {
                continue;
            };
            let Some(class_name) = constructor.qualified_name.strip_suffix(".constructor") else {
                continue;
            };
            if let Some(class) = scan.candidates.iter().find(|candidate| {
                candidate.path == constructor.path
                    && candidate.kind == "class"
                    && candidate.name == class_name
            }) {
                constructors_by_class
                    .entry(class.id.clone())
                    .or_default()
                    .push(owner.id.clone());
            }
        }
        for constructors in constructors_by_class.values_mut() {
            constructors.sort();
            constructors.dedup();
        }

        let mut edges_by_source = HashMap::<String, Vec<Edge>>::new();
        let mut seen = HashSet::new();
        for edge in &scan.edges {
            let edge = normalize_graph_edge(edge);
            if seen.insert(edge.clone()) {
                edges_by_source
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge);
            }
        }
        for edge in semantic_edges {
            let edge = normalize_semantic_edge(edge);
            if seen.insert(edge.clone()) {
                edges_by_source
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge);
            }
        }
        for edges in edges_by_source.values_mut() {
            edges.sort_by(|left, right| {
                (&left.target, &left.kind, left.mode).cmp(&(&right.target, &right.kind, right.mode))
            });
        }

        Self {
            scan,
            candidates,
            owners,
            owners_by_parent,
            constructors_by_class,
            edges_by_source,
            runtime_queue: VecDeque::new(),
            type_queue: VecDeque::new(),
            active_runtime_owners: BTreeSet::new(),
            active_type_owners: BTreeSet::new(),
            result: Reachability::default(),
        }
    }

    fn seed_runtime_root(&mut self, root: &str, consumer: bool) {
        if self.candidates.contains_key(root) {
            self.mark_candidate(root, None, false);
            self.activate_called_candidate(root, false);
            if self.edges_by_source.contains_key(root) {
                self.enqueue_runtime_owner(root);
            }
            return;
        }
        if self.owners.contains_key(root) {
            self.enqueue_runtime_owner(root);
            if self
                .owners
                .get(root)
                .is_some_and(|index| self.scan.execution_owners[*index].kind == "class")
            {
                self.activate_instance_children(root);
            }
            return;
        }
        let path = file_path(root);
        self.activate_runtime_file(&path);
        if consumer {
            let owners = self
                .scan
                .execution_owners
                .iter()
                .filter(|owner| owner.path == path && owner.kind != "file-top-level")
                .map(|owner| owner.id.clone())
                .collect::<Vec<_>>();
            for owner in owners {
                self.enqueue_runtime_owner(&owner);
            }
        }
    }

    fn seed_type_root(&mut self, root: &str) {
        if self.candidates.contains_key(root) {
            self.mark_candidate(root, None, false);
            return;
        }
        if self.owners.contains_key(root) {
            self.enqueue_type_owner(root);
            return;
        }
        let file_id = format!("file::{}", file_path(root));
        if self.active_type_owners.insert(file_id.clone()) {
            self.type_queue.push_back(file_id);
        }
    }

    fn run(&mut self) {
        while !self.runtime_queue.is_empty() || !self.type_queue.is_empty() {
            while let Some(owner_id) = self.runtime_queue.pop_front() {
                self.process_runtime_owner(&owner_id);
            }
            while let Some(owner_id) = self.type_queue.pop_front() {
                self.process_type_owner(&owner_id);
            }
        }
    }

    fn process_runtime_owner(&mut self, owner_id: &str) {
        let Some(edges) = self.edges_by_source.get(owner_id).cloned() else {
            return;
        };
        for edge in edges {
            self.process_edge(owner_id, &edge, true);
        }
    }

    fn process_type_owner(&mut self, owner_id: &str) {
        let Some(edges) = self.edges_by_source.get(owner_id).cloned() else {
            return;
        };
        for edge in edges {
            if edge.mode == EdgeMode::Type {
                self.process_edge(owner_id, &edge, false);
            }
        }
    }

    fn process_edge(&mut self, source: &str, edge: &Edge, allow_runtime: bool) {
        let source_path = self.node_path(source);
        let target_path = self.node_path(&edge.target);
        let target_candidate = self.candidates.get(&edge.target).copied();
        let runtime_edge = allow_runtime && edge.mode == EdgeMode::Runtime;

        if is_reexport(&edge.kind) {
            if !allow_runtime && edge.mode == EdgeMode::Type {
                if let Some(target_path) = target_path {
                    self.activate_type_file(&target_path);
                }
            } else if allow_runtime
                && edge.mode == EdgeMode::Runtime
                && let Some(path) = target_path
                && self.scan.file_effects.get(&path).map(String::as_str) == Some("side-effectful")
            {
                self.activate_runtime_file(&path);
            }
            return;
        }

        if let Some(target_path) = target_path.as_deref() {
            if edge.mode == EdgeMode::Type {
                self.activate_type_file(target_path);
            } else if runtime_edge && is_file_traversal(&edge.kind) {
                self.activate_runtime_file(target_path);
            }
        }

        if let Some(index) = target_candidate {
            let target = &self.scan.candidates[index];
            if edge.mode == EdgeMode::Type {
                self.mark_candidate(&edge.target, source_path.as_deref(), true);
            } else if runtime_edge && is_symbol_evidence(&edge.kind) {
                self.mark_candidate(&edge.target, source_path.as_deref(), true);
                if is_activation_edge(&edge.kind) || target.callable {
                    self.activate_called_candidate(&edge.target, edge.kind == "instantiates");
                } else if self.edges_by_source.contains_key(&edge.target) {
                    self.enqueue_runtime_owner(&edge.target);
                }
            } else if runtime_edge && target.kind == "constructor" {
                self.activate_called_candidate(&edge.target, true);
            }
        } else if runtime_edge
            && is_activation_edge(&edge.kind)
            && self.owners.contains_key(&edge.target)
        {
            self.enqueue_runtime_owner(&edge.target);
            if edge.kind == "instantiates" {
                self.activate_instance_children(&edge.target);
            }
        }
    }

    fn activate_runtime_file(&mut self, path: &str) {
        self.result.used_files.insert(path.to_owned());
        self.result.runtime_used_files.insert(path.to_owned());
        let file_id = format!("file::{path}");
        let first_activation = self.active_runtime_owners.insert(file_id.clone());
        if first_activation {
            self.runtime_queue.push_back(file_id.clone());
            self.activate_runtime_children(&file_id);
        }
    }

    fn activate_type_file(&mut self, path: &str) {
        self.result.used_files.insert(path.to_owned());
        let file_id = format!("file::{path}");
        if self.active_type_owners.insert(file_id.clone()) {
            self.type_queue.push_back(file_id);
        }
    }

    fn activate_runtime_children(&mut self, parent_id: &str) {
        let Some(children) = self.owners_by_parent.get(parent_id).cloned() else {
            return;
        };
        for owner_id in children {
            let Some(owner_index) = self.owners.get(&owner_id).copied() else {
                continue;
            };
            let owner = &self.scan.execution_owners[owner_index];
            let activate = match owner.kind.as_str() {
                "iife" | "static-block" | "static-field-initializer" => true,
                "class" => true,
                "variable-initializer" => owner
                    .finding_candidate_id
                    .as_deref()
                    .and_then(|id| self.candidates.get(id).copied())
                    .is_none_or(|index| {
                        let candidate = &self.scan.candidates[index];
                        !candidate.callable || candidate.initializer_effect != "side-effect-free"
                    }),
                _ => false,
            };
            if activate {
                self.enqueue_runtime_owner(&owner_id);
            }
        }
    }

    fn enqueue_runtime_owner(&mut self, owner_id: &str) {
        let is_source = self.owners.contains_key(owner_id)
            || self.candidates.contains_key(owner_id)
            || self.edges_by_source.contains_key(owner_id);
        if is_source && self.active_runtime_owners.insert(owner_id.to_owned()) {
            self.runtime_queue.push_back(owner_id.to_owned());
            if self.owners.contains_key(owner_id) {
                self.activate_runtime_children(owner_id);
            }
        }
    }

    fn enqueue_type_owner(&mut self, owner_id: &str) {
        if self.owners.contains_key(owner_id) && self.active_type_owners.insert(owner_id.to_owned())
        {
            self.type_queue.push_back(owner_id.to_owned());
        }
    }

    fn mark_candidate(&mut self, id: &str, source_path: Option<&str>, local: bool) {
        self.result.used_candidates.insert(id.to_owned());
        let Some(index) = self.candidates.get(id).copied() else {
            return;
        };
        let candidate = &self.scan.candidates[index];
        if source_path.is_some_and(|path| path == candidate.path) && local {
            self.result.local_used_candidates.insert(id.to_owned());
        }
        if source_path.is_none_or(|path| path != candidate.path) {
            self.result.used_files.insert(candidate.path.clone());
        }
    }

    fn activate_called_candidate(&mut self, id: &str, instantiate: bool) {
        let Some(index) = self.candidates.get(id).copied() else {
            return;
        };
        let candidate = self.scan.candidates[index].clone();
        if let Some(initializer_owner) = candidate.initializer_owner.as_deref() {
            self.enqueue_runtime_owner(initializer_owner);
        }
        if self.owners.contains_key(id) {
            self.enqueue_runtime_owner(id);
        }
        if instantiate || candidate.kind == "class" || candidate.kind == "constructor" {
            if let Some(constructors) = self.constructors_by_class.get(id).cloned() {
                for constructor in constructors {
                    self.enqueue_runtime_owner(&constructor);
                }
            }
            self.activate_instance_children(id);
        }
    }

    fn activate_instance_children(&mut self, id: &str) {
        if let Some(children) = self.owners_by_parent.get(id).cloned() {
            for child in children {
                if self.owners.get(&child).is_some_and(|index| {
                    matches!(
                        self.scan.execution_owners[*index].kind.as_str(),
                        "constructor" | "field-initializer"
                    )
                }) {
                    self.enqueue_runtime_owner(&child);
                }
            }
        }
    }

    fn node_path(&self, id: &str) -> Option<String> {
        if let Some(index) = self.candidates.get(id).copied() {
            return Some(self.scan.candidates[index].path.clone());
        }
        if let Some(index) = self.owners.get(id).copied() {
            return Some(self.scan.execution_owners[index].path.clone());
        }
        id.strip_prefix("file::").map(str::to_owned)
    }
}

fn normalize_graph_edge(edge: &GraphEdge) -> Edge {
    Edge {
        source: edge.source.clone(),
        target: edge.target.clone(),
        kind: normalize_kind(&edge.kind),
        mode: normalize_mode(&edge.kind, &edge.mode),
    }
}

fn normalize_semantic_edge(edge: &SemanticEdge) -> Edge {
    Edge {
        source: edge.source.clone(),
        target: edge.target.clone(),
        kind: normalize_kind(&edge.kind),
        mode: normalize_mode(&edge.kind, &edge.mode),
    }
}

fn normalize_kind(kind: &str) -> String {
    match kind {
        "calls" => "call".to_owned(),
        "imports" => "import".to_owned(),
        "references" => "reference".to_owned(),
        "instantiate" => "instantiates".to_owned(),
        "re-export" | "reexports" => "reexport".to_owned(),
        other => other.to_owned(),
    }
}

fn normalize_mode(kind: &str, mode: &str) -> EdgeMode {
    if mode == "type" || kind == "type-import" {
        EdgeMode::Type
    } else {
        EdgeMode::Runtime
    }
}

fn is_activation_edge(kind: &str) -> bool {
    matches!(kind, "call" | "instantiates" | "callback" | "immediate")
}

fn is_file_traversal(kind: &str) -> bool {
    matches!(kind, "import" | "dynamic-import" | "reference" | "require")
}

fn is_reexport(kind: &str) -> bool {
    kind == "reexport"
}

fn is_symbol_evidence(kind: &str) -> bool {
    matches!(
        kind,
        "call" | "callback" | "immediate" | "instantiates" | "reference"
    )
}

fn file_path(root: &str) -> String {
    root.strip_prefix("file::")
        .unwrap_or(root)
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unused::oxc::{Candidate, ExecutionOwner};

    fn candidate(id: &str, kind: &str, name: &str, path: &str) -> Candidate {
        Candidate {
            id: id.to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            path: path.to_owned(),
            start: 0,
            line: 1,
            column: 1,
            local_used: false,
            exported: false,
            reexport_locations: Vec::new(),
            unknown: false,
            coverage_reason: None,
            language: "typescript".to_owned(),
            qualified_name: name.to_owned(),
            top_level: true,
            callable: kind == "function" || kind == "class",
            initializer_owner: None,
            initializer_effect: "none".to_owned(),
        }
    }

    fn owner(
        id: &str,
        kind: &str,
        path: &str,
        candidate_id: Option<&str>,
        parent: &str,
    ) -> ExecutionOwner {
        ExecutionOwner {
            id: id.to_owned(),
            kind: kind.to_owned(),
            path: path.to_owned(),
            start: 0,
            line: 1,
            column: 1,
            finding_candidate_id: candidate_id.map(str::to_owned),
            parent_id: Some(parent.to_owned()),
        }
    }

    fn edge(source: &str, target: &str, kind: &str, mode: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_owned(),
            target: target.to_owned(),
            kind: kind.to_owned(),
            path: "src/main.ts".to_owned(),
            start: 0,
            line: 1,
            column: 1,
            confidence: "exact".to_owned(),
            mode: mode.to_owned(),
            provenance: "test".to_owned(),
        }
    }

    fn scan_with(
        candidates: Vec<Candidate>,
        owners: Vec<ExecutionOwner>,
        edges: Vec<GraphEdge>,
        used_files: &[&str],
    ) -> ScanResult {
        ScanResult {
            candidates,
            execution_owners: owners,
            edges,
            used_files: used_files.iter().map(|path| (*path).to_owned()).collect(),
            ..ScanResult::default()
        }
    }

    #[test]
    fn dead_function_cycle_does_not_propagate() {
        let a = candidate("a", "function", "a", "src/dead.ts");
        let b = candidate("b", "function", "b", "src/dead.ts");
        let owners = vec![
            owner(
                "a",
                "function",
                "src/dead.ts",
                Some("a"),
                "file::src/dead.ts",
            ),
            owner(
                "b",
                "function",
                "src/dead.ts",
                Some("b"),
                "file::src/dead.ts",
            ),
        ];
        let scan = scan_with(
            vec![a, b],
            owners,
            vec![
                edge("a", "b", "call", "runtime"),
                edge("b", "a", "call", "runtime"),
            ],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::new(),
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(result.used_candidates.is_empty());
    }

    #[test]
    fn incomplete_entrypoints_do_not_turn_dead_local_cycle_into_usage() {
        let a = candidate("a", "function", "a", "src/dead.ts");
        let b = candidate("b", "function", "b", "src/dead.ts");
        let owners = vec![
            owner(
                "a",
                "function",
                "src/dead.ts",
                Some("a"),
                "file::src/dead.ts",
            ),
            owner(
                "b",
                "function",
                "src/dead.ts",
                Some("b"),
                "file::src/dead.ts",
            ),
        ];
        let scan = scan_with(
            vec![a, b],
            owners,
            vec![
                edge("a", "b", "call", "runtime"),
                edge("b", "a", "call", "runtime"),
            ],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::new(),
            false,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(result.used_candidates.is_empty());
    }

    #[test]
    fn reachable_function_cycle_propagates_from_file_root() {
        let a = candidate("a", "function", "a", "src/main.ts");
        let b = candidate("b", "function", "b", "src/main.ts");
        let owners = vec![
            owner(
                "a",
                "function",
                "src/main.ts",
                Some("a"),
                "file::src/main.ts",
            ),
            owner(
                "b",
                "function",
                "src/main.ts",
                Some("b"),
                "file::src/main.ts",
            ),
        ];
        let scan = scan_with(
            vec![a, b],
            owners,
            vec![
                edge("file::src/main.ts", "a", "call", "runtime"),
                edge("a", "b", "call", "runtime"),
                edge("b", "a", "call", "runtime"),
            ],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::from(["src/main.ts".to_owned()]),
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert_eq!(
            result.used_candidates,
            BTreeSet::from(["a".to_owned(), "b".to_owned()])
        );
    }

    #[test]
    fn dead_file_cycle_does_not_propagate() {
        let scan = scan_with(
            Vec::new(),
            Vec::new(),
            vec![
                edge("file::src/a.ts", "file::src/b.ts", "import", "runtime"),
                edge("file::src/b.ts", "file::src/a.ts", "import", "runtime"),
            ],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::new(),
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(result.used_files.is_empty());
    }

    #[test]
    fn type_edge_protects_target_without_runtime_activation() {
        let typed = candidate("typed", "function", "Typed", "src/types.ts");
        let dependency = candidate("dependency", "function", "dependency", "src/types.ts");
        let owners = vec![
            owner(
                "typed",
                "function",
                "src/types.ts",
                Some("typed"),
                "file::src/types.ts",
            ),
            owner(
                "dependency",
                "function",
                "src/types.ts",
                Some("dependency"),
                "file::src/types.ts",
            ),
        ];
        let scan = scan_with(
            vec![typed, dependency],
            owners,
            vec![
                edge("file::src/consumer.d.ts", "typed", "reference", "type"),
                edge("typed", "dependency", "call", "runtime"),
            ],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::new(),
            true,
            &BTreeSet::new(),
            &BTreeSet::from(["src/consumer.d.ts".to_owned()]),
        );
        assert!(result.used_candidates.contains("typed"));
        assert!(!result.used_candidates.contains("dependency"));
        assert!(result.used_files.contains("src/types.ts"));
    }

    #[test]
    fn type_import_chain_propagates_through_nested_files() {
        let scan = scan_with(
            Vec::new(),
            Vec::new(),
            vec![
                edge(
                    "file::src/main.ts",
                    "file::src/root.ts",
                    "type-import",
                    "type",
                ),
                edge("file::src/root.ts", "file::src/leaf.ts", "reexport", "type"),
            ],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::from(["src/main.ts".to_owned()]),
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );

        assert_eq!(
            result.used_files,
            BTreeSet::from([
                "src/main.ts".to_owned(),
                "src/root.ts".to_owned(),
                "src/leaf.ts".to_owned(),
            ])
        );
    }

    #[test]
    fn runtime_root_does_not_follow_bare_type_reexport() {
        let scan = scan_with(
            Vec::new(),
            Vec::new(),
            vec![edge(
                "file::src/main.ts",
                "file::src/types.ts",
                "reexport",
                "type",
            )],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::from(["src/main.ts".to_owned()]),
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );

        assert!(!result.used_files.contains("src/types.ts"));
    }

    #[test]
    fn commonjs_require_activates_target_file() {
        let scan = scan_with(
            Vec::new(),
            Vec::new(),
            vec![edge(
                "file::src/main.cjs",
                "file::src/required.cjs",
                "require",
                "runtime",
            )],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::from(["src/main.cjs".to_owned()]),
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );

        assert!(result.used_files.contains("src/required.cjs"));
    }

    #[test]
    fn call_activates_callable_initializer_and_constructor_owner() {
        let mut factory = candidate("factory", "variable", "factory", "src/main.ts");
        factory.callable = true;
        factory.initializer_owner = Some("factory-init".to_owned());
        factory.initializer_effect = "side-effect-free".to_owned();
        let mut thing = candidate("thing", "class", "Thing", "src/main.ts");
        thing.callable = false;
        let constructor = candidate("constructor", "constructor", "constructor", "src/main.ts");
        let dependency = candidate("dependency", "function", "dependency", "src/main.ts");
        let owners = vec![
            owner(
                "factory-init",
                "variable-initializer",
                "src/main.ts",
                Some("factory"),
                "file::src/main.ts",
            ),
            owner(
                "thing",
                "class",
                "src/main.ts",
                Some("thing"),
                "file::src/main.ts",
            ),
            owner(
                "constructor",
                "constructor",
                "src/main.ts",
                Some("constructor"),
                "thing",
            ),
        ];
        let scan = scan_with(
            vec![factory, thing, constructor, dependency],
            owners,
            vec![
                edge("file::src/main.ts", "factory", "call", "runtime"),
                edge("factory-init", "dependency", "call", "runtime"),
                edge("file::src/main.ts", "thing", "instantiates", "runtime"),
                edge("constructor", "dependency", "call", "runtime"),
            ],
            &[],
        );
        let result = compute(
            &scan,
            &[],
            &BTreeSet::from(["src/main.ts".to_owned()]),
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(result.used_candidates.contains("factory"));
        assert!(result.used_candidates.contains("thing"));
        assert!(result.used_candidates.contains("dependency"));
    }
}
