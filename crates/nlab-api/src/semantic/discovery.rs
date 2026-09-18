use super::*;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteCall {
    pub interface: String,
    pub method: String,
    pub arity: Option<usize>,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub chain: Vec<String>,
}

pub(crate) struct RepositoryWalk {
    pub calls: Vec<RemoteCall>,
    pub visited_methods: usize,
    pub unresolved_calls: usize,
    pub gaps: Vec<String>,
}

impl SemanticAnalyzer<'_> {
    /// Discovery follows typed calls only; same-name graph fallback is not repository evidence.
    pub(crate) fn discover_calls(
        &mut self,
        roots: &[String],
        remote_types: &BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<RepositoryWalk> {
        let graph = self.project.graph();
        let mut queue = roots
            .iter()
            .map(|id| (id.clone(), Vec::new()))
            .collect::<VecDeque<_>>();
        let previous_count = visited.len();
        let mut output = RepositoryWalk {
            calls: Vec::new(),
            visited_methods: 0,
            unresolved_calls: 0,
            gaps: Vec::new(),
        };
        while let Some((id, mut chain)) = queue.pop_front() {
            if visited.contains(&id) {
                continue;
            }
            if visited.len() >= 25_000 {
                output
                    .gaps
                    .push("method traversal limit reached (25000)".to_owned());
                break;
            }
            visited.insert(id.clone());
            let method = graph
                .nodes
                .get(&id)
                .context("discovery method missing from index")?;
            if serde_json::from_str::<Vec<String>>(&method.decorators)
                .is_ok_and(|decorators| decorators.iter().any(|name| name == "lombok"))
            {
                continue;
            }
            chain.push(method.qualified_name.replace("::", "."));
            let parsed = self.parsed(&method.file_path)?;
            let Some(declaration) = lookup::method_declaration(parsed, method) else {
                output.gaps.push(format!(
                    "indexed method has no source declaration: {}:{} ({})",
                    method.file_path, method.start_line, method.name
                ));
                continue;
            };
            let has_body = declaration.child_by_field_name("body").is_some();
            if let Some(implementation) = self.implementation_method(method) {
                queue.push_back((implementation, chain.clone()));
            } else if !has_body
                && graph.edges.iter().any(|edge| {
                    edge.kind == "contains"
                        && edge.target == id
                        && graph
                            .nodes
                            .get(&edge.source)
                            .is_some_and(|owner| owner.kind == "interface")
                })
            {
                output.gaps.push(format!(
                    "no unique implementation: {}",
                    method.qualified_name
                ));
            }
            for invocation in self.method_invocations(method)? {
                let receiver = invocation
                    .receiver
                    .as_deref()
                    .unwrap_or("")
                    .trim_start_matches("this.");
                let receiver_type = self.receiver_type(method, receiver, invocation.offset)?;
                let imported = receiver_type
                    .as_deref()
                    .and_then(parse_java_type)
                    .and_then(|kind| self.project.imported_type(&method.file_path, &kind.name));
                if let Some(interface) = imported
                    .as_ref()
                    .filter(|name| remote_types.contains(*name))
                {
                    output.calls.push(RemoteCall {
                        interface: interface.clone(),
                        method: invocation.name,
                        arity: invocation.exact_arity.then_some(invocation.arity),
                        file: method.file_path.clone(),
                        line: invocation.line,
                        column: invocation.column + 1,
                        chain: chain.clone(),
                    });
                    continue;
                }
                if imported
                    .as_deref()
                    .is_some_and(|name| self.project.node_for_fqn(name).is_none())
                {
                    output.unresolved_calls += 1;
                    continue;
                }
                let targets = self.resolve_invocation(method, &invocation)?;
                if targets.is_empty() {
                    output.unresolved_calls += 1;
                }
                for target in targets {
                    queue.push_back((target, chain.clone()));
                }
            }
        }
        output.visited_methods = visited.len() - previous_count;
        output.calls.sort_by(|left, right| {
            (
                &left.file,
                left.line,
                left.column,
                &left.interface,
                &left.method,
            )
                .cmp(&(
                    &right.file,
                    right.line,
                    right.column,
                    &right.interface,
                    &right.method,
                ))
        });
        output.gaps.sort();
        output.gaps.dedup();
        Ok(output)
    }
}
