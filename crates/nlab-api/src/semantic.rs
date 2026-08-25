use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser, Tree};

use super::graph::{GraphEdge, GraphNode, Reachability, Snapshot};
use super::java::{JavaProject, parse_java_type};
use super::model::{
    CodedValue, FieldTarget, LinkedEnum, Operation, ProvenanceStatus, Schema, SemanticPatch,
    ServiceOwner, TypeRef, WireValue,
};

const MAX_RESPONSE_DEPTH: usize = 32;

#[derive(Clone, Debug, Default)]
struct Domain {
    enum_fqn: Option<String>,
    enum_source: Option<String>,
    accessor: Option<String>,
    values: Vec<CodedValue>,
    complete: bool,
    external: BTreeSet<String>,
    unknown: BTreeSet<String>,
    literals: BTreeSet<String>,
    transformed: bool,
    evidence: Vec<String>,
}

#[derive(Clone, Debug)]
enum Expression {
    Getter { receiver: String, accessor: String },
    Call { name: String, source: String },
    Identifier(String),
    Literal(String),
    Branch(Vec<Expression>),
    Transformed(String),
    Unknown(String),
}

#[derive(Clone, Debug)]
struct InvocationSite {
    name: String,
    receiver: Option<String>,
    arity: usize,
    exact_arity: bool,
    arguments: Vec<Expression>,
    offset: usize,
    line: usize,
    column: usize,
}

struct ParsedFile {
    source: String,
    tree: Tree,
}

pub struct SemanticAnalyzer<'a> {
    project: &'a JavaProject<'a>,
    parsed_files: HashMap<String, ParsedFile>,
    enum_cache: HashMap<(String, String), Domain>,
    call_target_cache: HashMap<String, Vec<String>>,
    invocation_cache: HashMap<String, Vec<InvocationSite>>,
    parameter_cache: HashMap<(String, String, usize), Domain>,
}

impl<'a> SemanticAnalyzer<'a> {
    pub fn new(project: &'a JavaProject<'a>) -> Self {
        Self {
            project,
            parsed_files: HashMap::new(),
            enum_cache: HashMap::new(),
            call_target_cache: HashMap::new(),
            invocation_cache: HashMap::new(),
            parameter_cache: HashMap::new(),
        }
    }

    pub fn enrich(
        &mut self,
        operations: &mut [Operation],
        schemas: &BTreeMap<String, Schema>,
    ) -> Result<()> {
        for operation in operations {
            let root = operation_root(self.project.graph(), operation)?;
            let reachable = self.build_reachability(root)?;
            operation.service = service_owner(self.project.graph(), root, &reachable);
            if operation.service.is_none() {
                operation
                    .warnings
                    .push("no unique delegated Service; using Facade output layout".to_owned());
            }
            operation.semantic_patches = self.operation_patches(operation, schemas, &reachable)?;
        }
        Ok(())
    }

    pub fn enrich_linked_enums(&self, schemas: &mut BTreeMap<String, Schema>) -> Result<()> {
        for schema in schemas.values_mut() {
            for field in &mut schema.fields {
                let Some(description) = field.description.as_deref() else {
                    continue;
                };
                let Some(enum_node) = linked_enum_nodes(self.project, description)
                    .into_iter()
                    .next()
                else {
                    continue;
                };
                for accessor in enum_accessor_candidates(&field.name) {
                    let domain = extract_enum_domain(self.project, enum_node, &accessor)?;
                    if domain.complete && !domain.values.is_empty() {
                        field.linked_enum = Some(LinkedEnum {
                            enum_fqn: enum_node.qualified_name.replace("::", "."),
                            enum_source: enum_node.file_path.clone(),
                            accessor,
                            values: domain.values,
                        });
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn build_reachability(&mut self, root: &GraphNode) -> Result<Reachability> {
        let graph = self.project.graph();
        let initial = graph.reachable_calls(&root.id)?;
        let mut nodes = initial.nodes;
        let mut parent = initial.parent;
        let mut queue = VecDeque::from(nodes.iter().cloned().collect::<Vec<_>>());
        while let Some(method_id) = queue.pop_front() {
            let method = graph
                .nodes
                .get(&method_id)
                .context("reachable method disappeared")?;
            let targets = if let Some(targets) = self.call_target_cache.get(&method_id) {
                targets.clone()
            } else {
                let mut targets = graph
                    .outgoing(&method_id)
                    .filter(|edge| edge.kind == "calls")
                    .map(|edge| edge.target.clone())
                    .collect::<BTreeSet<_>>();
                for invocation in self.method_invocations(method)? {
                    targets.extend(self.resolve_invocation(method, &invocation)?);
                }
                let targets = targets.into_iter().collect::<Vec<_>>();
                self.call_target_cache
                    .insert(method_id.clone(), targets.clone());
                targets
            };
            for target in targets {
                if nodes.insert(target.clone()) {
                    parent.insert(target.clone(), method_id.clone());
                    queue.push_back(target);
                    if nodes.len() > 25_000 {
                        anyhow::bail!("operation call graph exceeded 25000 nodes");
                    }
                }
            }
        }
        Ok(Reachability { nodes, parent })
    }

    fn method_invocations(&mut self, method: &GraphNode) -> Result<Vec<InvocationSite>> {
        if let Some(invocations) = self.invocation_cache.get(&method.id) {
            return Ok(invocations.clone());
        }
        let parsed = self.parsed(&method.file_path)?;
        let declaration = descendants(parsed.tree.root_node())
            .into_iter()
            .filter(|node| {
                matches!(
                    node.kind(),
                    "method_declaration" | "constructor_declaration"
                )
            })
            .find(|node| {
                node.start_position().row + 1 == method.start_line
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|name| text_of(&parsed.source, name) == method.name)
            });
        let Some(declaration) = declaration else {
            self.invocation_cache.insert(method.id.clone(), Vec::new());
            return Ok(Vec::new());
        };
        let invocations = descendants(declaration)
            .into_iter()
            .filter(|node| matches!(node.kind(), "method_invocation" | "method_reference"))
            .map(|node| {
                let reference_children = if node.kind() == "method_reference" {
                    named_children(node)
                } else {
                    Vec::new()
                };
                let arguments = node
                    .child_by_field_name("arguments")
                    .map(named_children)
                    .unwrap_or_default();
                InvocationSite {
                    name: node
                        .child_by_field_name("name")
                        .or_else(|| reference_children.last().copied())
                        .map(|name| text_of(&parsed.source, name).to_owned())
                        .unwrap_or_default(),
                    receiver: node
                        .child_by_field_name("object")
                        .or_else(|| reference_children.first().copied())
                        .map(|object| text_of(&parsed.source, object).trim().to_owned()),
                    arity: arguments.len(),
                    exact_arity: node.kind() == "method_invocation",
                    arguments: arguments
                        .into_iter()
                        .map(|argument| expression_from_node(&parsed.source, argument))
                        .collect(),
                    offset: node.start_byte(),
                    line: node.start_position().row + 1,
                    column: node.start_position().column,
                }
            })
            .collect::<Vec<_>>();
        self.invocation_cache
            .insert(method.id.clone(), invocations.clone());
        Ok(invocations)
    }

    fn resolve_invocation(
        &mut self,
        method: &GraphNode,
        invocation: &InvocationSite,
    ) -> Result<Vec<String>> {
        if invocation.name.is_empty() {
            return Ok(Vec::new());
        }
        let owner_fqn = method
            .qualified_name
            .rsplit_once("::")
            .map(|(owner, _)| owner)
            .unwrap_or(&method.qualified_name);
        let owner = if let Some(receiver) = invocation.receiver.as_deref() {
            let receiver = receiver.strip_prefix("this.").unwrap_or(receiver);
            if matches!(receiver, "this" | "super") {
                self.project.node_for_fqn(&owner_fqn.replace("::", "."))
            } else {
                if receiver.contains(['(', ')']) {
                    return Ok(Vec::new());
                }
                let type_name = self.receiver_type(method, receiver, invocation.offset)?;
                let Some(type_name) = type_name else {
                    return Ok(Vec::new());
                };
                let Some(type_ref) = parse_java_type(&type_name) else {
                    return Ok(Vec::new());
                };
                self.project
                    .resolve_type(&method.file_path, owner_fqn, &type_ref)
            }
        } else {
            self.project.node_for_fqn(&owner_fqn.replace("::", "."))
        };
        let Some(owner) = owner else {
            return Ok(Vec::new());
        };
        let mut candidates = self
            .project
            .graph()
            .contained(&owner.id, "method")
            .into_iter()
            .filter(|target| {
                target.name == invocation.name
                    && (!invocation.exact_arity
                        || signature_arity(&target.signature)
                            .is_none_or(|arity| arity == invocation.arity))
            })
            .map(|target| target.id.clone())
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        Ok(if candidates.len() == 1 {
            candidates
        } else {
            Vec::new()
        })
    }

    fn receiver_type(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        before_offset: usize,
    ) -> Result<Option<String>> {
        if receiver
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_uppercase())
        {
            return Ok(Some(receiver.to_owned()));
        }
        if let Some(type_name) = method_parameters(&method.signature)
            .into_iter()
            .find_map(|(type_name, name)| (name == receiver).then_some(type_name))
        {
            return Ok(Some(type_name));
        }
        let owner_fqn = method
            .qualified_name
            .rsplit_once("::")
            .map(|(owner, _)| owner.replace("::", "."));
        if let Some(owner) = owner_fqn
            .as_deref()
            .and_then(|owner| self.project.node_for_fqn(owner))
        {
            if let Some(field) = self
                .project
                .graph()
                .contained(&owner.id, "field")
                .into_iter()
                .find(|field| field.name == receiver)
            {
                return Ok(declared_variable_type(&field.signature, &field.name));
            }
        }
        let parsed = self.parsed(&method.file_path)?;
        let local = descendants(parsed.tree.root_node())
            .into_iter()
            .rfind(|node| {
                node.kind() == "variable_declarator"
                    && node.start_byte() < before_offset
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|name| text_of(&parsed.source, name) == receiver)
            })
            .and_then(|variable| {
                let declaration = variable.parent()?;
                declaration
                    .child_by_field_name("type")
                    .map(|type_node| text_of(&parsed.source, type_node).to_owned())
            });
        Ok(local)
    }

    fn operation_patches(
        &mut self,
        operation: &Operation,
        schemas: &BTreeMap<String, Schema>,
        reachable: &Reachability,
    ) -> Result<Vec<SemanticPatch>> {
        let mut patches = Vec::new();
        for (schema_fqn, prefix) in response_schema_paths(&operation.response, schemas) {
            let Some(schema) = schemas.get(&schema_fqn) else {
                continue;
            };
            let Some(class) = self.project.node_for_fqn(&schema_fqn) else {
                continue;
            };
            for field in &schema.fields {
                if !is_scalar(&field.java_type) {
                    continue;
                }
                let field_path = if prefix.is_empty() {
                    field.name.clone()
                } else {
                    format!("{prefix}.{}", field.name)
                };
                patches.push(self.field_patch(
                    operation,
                    class,
                    &field.name,
                    field_path,
                    reachable,
                )?);
            }
        }
        patches.sort_by(|left, right| left.target.cmp(&right.target));
        patches.dedup_by(|left, right| left.target == right.target);
        Ok(patches)
    }

    fn field_patch(
        &mut self,
        operation: &Operation,
        class: &GraphNode,
        field_name: &str,
        field_path: String,
        reachable: &Reachability,
    ) -> Result<SemanticPatch> {
        let graph = self.project.graph();
        let setter_name = format!("set{}", uppercase_first(field_name));
        let setter = graph
            .contained(&class.id, "method")
            .into_iter()
            .find(|method| method.name == setter_name);
        let target = FieldTarget {
            operation_key: operation.key.clone(),
            schema_fqn: class.qualified_name.replace("::", "."),
            field_path,
            field_name: field_name.to_owned(),
        };
        let Some(setter) = setter else {
            return Ok(unresolved_patch(target, "setter not indexed"));
        };

        let mut domains = Vec::new();
        let write_edges = graph
            .incoming_calls(&setter.id)
            .filter(|edge| reachable.nodes.contains(&edge.source))
            .cloned()
            .collect::<Vec<_>>();
        let known_sites = write_edges
            .iter()
            .map(|edge| (edge.source.clone(), edge.line))
            .collect::<BTreeSet<_>>();
        for edge in write_edges {
            let writer = graph
                .nodes
                .get(&edge.source)
                .context("CodeGraph writer disappeared")?;
            let Some((expression, source)) = self.setter_argument(&edge, &setter.name)? else {
                let mut domain = Domain::default();
                domain.unknown.insert(format!(
                    "unindexed setter argument at {}:{}:{}",
                    writer.file_path, edge.line, edge.column
                ));
                domains.push(domain);
                continue;
            };
            let mut domain = self.analyze_expression(
                &operation.key,
                writer,
                expression,
                reachable,
                &mut BTreeSet::new(),
            )?;
            push_unique(
                &mut domain.evidence,
                format!(
                    "write:{}:{}:{}:{}",
                    writer.file_path, edge.line, edge.column, source
                ),
            );
            push_unique(
                &mut domain.evidence,
                format!("chain:{}", render_path(graph, reachable, &writer.id)),
            );
            domains.push(domain);
        }
        for gap in self.unindexed_setter_calls(reachable, &setter.name, &known_sites)? {
            let mut domain = Domain::default();
            domain.unknown.insert(gap);
            domains.push(domain);
        }
        for unresolved in reachable
            .nodes
            .iter()
            .flat_map(|id| graph.unresolved(id))
            .filter(|item| item.name == setter.name)
        {
            let mut domain = Domain::default();
            domain.unknown.insert(format!(
                "unresolved setter call:{}:{}:{}",
                unresolved.file_path, unresolved.line, unresolved.column
            ));
            domains.push(domain);
        }
        if domains.is_empty() {
            return Ok(unresolved_patch(
                target,
                "no operation-reachable write site",
            ));
        }
        Ok(classify_patch(target, domains))
    }

    fn unindexed_setter_calls(
        &mut self,
        reachable: &Reachability,
        setter_name: &str,
        known_sites: &BTreeSet<(String, usize)>,
    ) -> Result<Vec<String>> {
        let mut gaps = BTreeSet::new();
        for method_id in &reachable.nodes {
            let Some(method) = self.project.graph().nodes.get(method_id) else {
                continue;
            };
            for invocation in self.method_invocations(method)? {
                if invocation.name == setter_name
                    && !known_sites.contains(&(method.id.clone(), invocation.line))
                {
                    gaps.insert(format!(
                        "unindexed setter call:{}:{}:{}",
                        method.file_path, invocation.line, invocation.column
                    ));
                }
            }
        }
        Ok(gaps.into_iter().collect())
    }

    fn setter_argument(
        &mut self,
        edge: &GraphEdge,
        setter_name: &str,
    ) -> Result<Option<(Expression, String)>> {
        let parsed = self.parsed(&self.project.graph().nodes[&edge.source].file_path)?;
        let mut candidates = descendants(parsed.tree.root_node())
            .into_iter()
            .filter(|node| node.kind() == "method_invocation")
            .filter(|node| node.start_position().row + 1 == edge.line)
            .filter(|node| {
                node.child_by_field_name("name")
                    .is_some_and(|name| text_of(&parsed.source, name) == setter_name)
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|node| node.start_position().column.abs_diff(edge.column));
        let Some(invocation) = candidates.first().copied() else {
            return Ok(None);
        };
        let arguments = invocation
            .child_by_field_name("arguments")
            .map(named_children)
            .unwrap_or_default();
        let Some(argument) = arguments.first().copied() else {
            return Ok(None);
        };
        let source = text_of(&parsed.source, argument).to_owned();
        Ok(Some((
            expression_from_node(&parsed.source, argument),
            source,
        )))
    }

    fn analyze_expression(
        &mut self,
        operation_key: &str,
        writer: &GraphNode,
        expression: Expression,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, usize)>,
    ) -> Result<Domain> {
        match expression {
            Expression::Getter { receiver, accessor } => {
                if let Some(enum_node) = self.enum_for_receiver(writer, &receiver).cloned() {
                    return self.enum_domain(&enum_node, &accessor);
                }
                let mut domain = Domain::default();
                domain
                    .unknown
                    .insert(format!("unknown getter receiver:{receiver}.{accessor}"));
                Ok(domain)
            }
            Expression::Call { name, source } => {
                let external = self
                    .project
                    .graph()
                    .outgoing(&writer.id)
                    .filter(|edge| edge.kind == "calls")
                    .filter_map(|edge| self.project.graph().nodes.get(&edge.target))
                    .filter(|target| target.name == name)
                    .any(|target| is_external_path(&target.file_path));
                let mut domain = Domain::default();
                if external {
                    domain.external.insert(source);
                } else {
                    domain.unknown.insert(source);
                }
                Ok(domain)
            }
            Expression::Identifier(value) => {
                if let Some(index) = method_parameters(&writer.signature)
                    .iter()
                    .position(|(_, name)| name == &value)
                {
                    return self.resolve_parameter_domain(
                        operation_key,
                        writer,
                        index,
                        reachable,
                        visiting,
                    );
                }
                let mut domain = Domain::default();
                domain.unknown.insert(value);
                Ok(domain)
            }
            Expression::Literal(value) => {
                let mut domain = Domain::default();
                domain.literals.insert(value);
                Ok(domain)
            }
            Expression::Branch(expressions) => {
                let mut result = Domain::default();
                for expression in expressions {
                    merge_domain(
                        &mut result,
                        self.analyze_expression(
                            operation_key,
                            writer,
                            expression,
                            reachable,
                            visiting,
                        )?,
                    );
                }
                Ok(result)
            }
            Expression::Transformed(value) => {
                let mut domain = Domain {
                    transformed: true,
                    ..Domain::default()
                };
                domain.unknown.insert(value);
                Ok(domain)
            }
            Expression::Unknown(value) => {
                let mut domain = Domain::default();
                domain.unknown.insert(value);
                Ok(domain)
            }
        }
    }

    fn resolve_parameter_domain(
        &mut self,
        operation_key: &str,
        method: &GraphNode,
        parameter_index: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, usize)>,
    ) -> Result<Domain> {
        let cache_key = (operation_key.to_owned(), method.id.clone(), parameter_index);
        if let Some(domain) = self.parameter_cache.get(&cache_key) {
            return Ok(domain.clone());
        }
        let visit_key = (method.id.clone(), parameter_index);
        if !visiting.insert(visit_key.clone()) {
            let mut domain = Domain::default();
            domain.unknown.insert(format!(
                "parameter cycle:{}[{parameter_index}]",
                method.qualified_name
            ));
            return Ok(domain);
        }

        let caller_ids = reachable
            .nodes
            .iter()
            .filter(|caller_id| *caller_id != &method.id)
            .cloned()
            .collect::<Vec<_>>();
        let mut domains = Vec::new();
        for caller_id in caller_ids {
            let Some(caller) = self.project.graph().nodes.get(&caller_id).cloned() else {
                continue;
            };
            for invocation in self.method_invocations(&caller)? {
                if !invocation.exact_arity || invocation.arguments.len() <= parameter_index {
                    continue;
                }
                if !self
                    .resolve_invocation(&caller, &invocation)?
                    .contains(&method.id)
                {
                    continue;
                }
                domains.push(self.analyze_expression(
                    operation_key,
                    &caller,
                    invocation.arguments[parameter_index].clone(),
                    reachable,
                    visiting,
                )?);
            }
        }
        visiting.remove(&visit_key);

        let domain = if domains.is_empty() {
            let mut domain = Domain::default();
            domain.unknown.insert(format!(
                "unbound parameter:{}[{parameter_index}]",
                method.qualified_name
            ));
            domain
        } else {
            let mut merged = Domain::default();
            for domain in domains {
                merge_domain(&mut merged, domain);
            }
            merged
        };
        self.parameter_cache.insert(cache_key, domain.clone());
        Ok(domain)
    }

    fn enum_for_receiver(&self, writer: &GraphNode, receiver: &str) -> Option<&GraphNode> {
        let owner = writer
            .qualified_name
            .rsplit_once("::")
            .map(|(owner, _)| owner)
            .unwrap_or(&writer.qualified_name);
        let receiver_root = receiver.split('.').next().unwrap_or(receiver);
        if let Some(type_name) = method_parameters(&writer.signature)
            .into_iter()
            .find_map(|(type_name, name)| (name == receiver_root).then_some(type_name))
        {
            let type_ref = parse_java_type(&type_name)?;
            let node = self
                .project
                .resolve_type(&writer.file_path, owner, &type_ref)?;
            return (node.kind == "enum").then_some(node);
        }
        let type_name = receiver_root
            .rsplit([':', '.'])
            .next()
            .unwrap_or(receiver_root);
        let type_ref = parse_java_type(type_name)?;
        let node = self
            .project
            .resolve_type(&writer.file_path, owner, &type_ref)?;
        (node.kind == "enum").then_some(node)
    }

    fn enum_domain(&mut self, enum_node: &GraphNode, accessor: &str) -> Result<Domain> {
        let key = (enum_node.id.clone(), accessor.to_owned());
        if let Some(domain) = self.enum_cache.get(&key) {
            return Ok(domain.clone());
        }
        let domain = extract_enum_domain(self.project, enum_node, accessor)?;
        self.enum_cache.insert(key, domain.clone());
        Ok(domain)
    }

    fn parsed(&mut self, file_path: &str) -> Result<&ParsedFile> {
        if !self.parsed_files.contains_key(file_path) {
            let source = self.project.source(file_path)?.to_owned();
            let mut parser = Parser::new();
            parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
            let tree = parser
                .parse(&source, None)
                .with_context(|| format!("parse Java source {file_path}"))?;
            self.parsed_files
                .insert(file_path.to_owned(), ParsedFile { source, tree });
        }
        Ok(&self.parsed_files[file_path])
    }
}

fn operation_root<'a>(graph: &'a Snapshot, operation: &Operation) -> Result<&'a GraphNode> {
    let candidates = graph
        .nodes
        .values()
        .filter(|node| {
            node.kind == "method"
                && node.file_path == operation.contract_source
                && node.name == operation.method_name
                && node.signature == operation.signature
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        anyhow::bail!(
            "operation identity is not unique: {} ({})",
            operation.key,
            candidates.len()
        );
    }
    Ok(candidates[0])
}

fn service_owner(
    graph: &Snapshot,
    root: &GraphNode,
    reachable: &Reachability,
) -> Option<ServiceOwner> {
    let mut distances = HashMap::from([(root.id.clone(), 0usize)]);
    let mut queue = VecDeque::from([root.id.clone()]);
    while let Some(current) = queue.pop_front() {
        let distance = distances[&current];
        for (child, parent) in &reachable.parent {
            if parent == &current && !distances.contains_key(child) {
                distances.insert(child.clone(), distance + 1);
                queue.push_back(child.clone());
            }
        }
    }
    let mut candidates = reachable
        .nodes
        .iter()
        .filter_map(|id| graph.nodes.get(id))
        .filter(|node| is_service_path(&node.file_path))
        .map(|node| (distances.get(&node.id).copied().unwrap_or(usize::MAX), node))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.qualified_name.cmp(&right.1.qualified_name))
    });
    let minimum = candidates.first()?.0;
    let nearest = candidates
        .into_iter()
        .take_while(|(distance, _)| *distance == minimum)
        .map(|(_, node)| node)
        .collect::<Vec<_>>();
    let owners = nearest
        .iter()
        .filter_map(|node| {
            node.qualified_name
                .rsplit_once("::")
                .map(|(owner, _)| owner)
        })
        .collect::<BTreeSet<_>>();
    if owners.len() != 1 {
        return None;
    }
    let method = nearest[0];
    let (owner, _) = method.qualified_name.rsplit_once("::")?;
    Some(ServiceOwner {
        class_name: owner.rsplit("::").next().unwrap_or(owner).to_owned(),
        class_fqn: owner.replace("::", "."),
        method_name: method.name.clone(),
        source_path: method.file_path.clone(),
    })
}

fn response_schema_paths(
    response: &TypeRef,
    schemas: &BTreeMap<String, Schema>,
) -> Vec<(String, String)> {
    let mut output = BTreeSet::new();
    let mut ancestors = BTreeSet::new();
    visit_type_paths(response, "", schemas, 0, &mut ancestors, &mut output);
    output.into_iter().collect()
}

fn visit_type_paths(
    type_ref: &TypeRef,
    path: &str,
    schemas: &BTreeMap<String, Schema>,
    depth: usize,
    ancestors: &mut BTreeSet<String>,
    output: &mut BTreeSet<(String, String)>,
) {
    if depth > MAX_RESPONSE_DEPTH {
        return;
    }
    let simple = type_ref.simple_name();
    if matches!(simple, "PageList" | "Page" | "PageResult") {
        let fqn = type_ref.name.replace("::", ".");
        if schemas.contains_key(&fqn) {
            output.insert((fqn, path.to_owned()));
        }
        if let Some(item) = type_ref.arguments.last() {
            visit_type_paths(
                item,
                &join_path(path, "list[]"),
                schemas,
                depth + 1,
                ancestors,
                output,
            );
        }
        return;
    }
    if is_collection(simple) || type_ref.array_depth > 0 {
        let item_path = if path.is_empty() {
            String::new()
        } else {
            format!("{path}[]")
        };
        if let Some(item) = type_ref.arguments.first() {
            visit_type_paths(item, &item_path, schemas, depth + 1, ancestors, output);
        } else {
            let mut item = type_ref.clone();
            item.array_depth = item.array_depth.saturating_sub(1);
            visit_type_paths(&item, &item_path, schemas, depth + 1, ancestors, output);
        }
        return;
    }
    if matches!(simple, "Map" | "HashMap" | "LinkedHashMap") {
        if let Some(value) = type_ref.arguments.last() {
            visit_type_paths(value, path, schemas, depth + 1, ancestors, output);
        }
        return;
    }
    let fqn = type_ref.name.replace("::", ".");
    let Some(schema) = schemas.get(&fqn) else {
        return;
    };
    if !ancestors.insert(fqn.clone()) {
        return;
    }
    output.insert((fqn.clone(), path.to_owned()));
    for field in &schema.fields {
        visit_type_paths(
            &field.java_type,
            &join_path(path, &field.name),
            schemas,
            depth + 1,
            ancestors,
            output,
        );
    }
    ancestors.remove(&fqn);
}

fn expression_from_node(source: &str, node: Node<'_>) -> Expression {
    let text = text_of(source, node).trim().to_owned();
    match node.kind() {
        "method_invocation" => {
            let name = node
                .child_by_field_name("name")
                .map(|name| text_of(source, name).to_owned())
                .unwrap_or_default();
            if getter_signal(&name).is_some() {
                let receiver = node
                    .child_by_field_name("object")
                    .map(|object| text_of(source, object).trim().to_owned())
                    .unwrap_or_default();
                Expression::Getter {
                    receiver,
                    accessor: name,
                }
            } else {
                Expression::Call { name, source: text }
            }
        }
        "identifier" | "field_access" => Expression::Identifier(text),
        kind if kind.ends_with("_literal") || matches!(kind, "true" | "false" | "null_literal") => {
            Expression::Literal(text)
        }
        "ternary_expression" => {
            let branches = ["consequence", "alternative"]
                .into_iter()
                .filter_map(|field| node.child_by_field_name(field))
                .map(|child| expression_from_node(source, child))
                .collect::<Vec<_>>();
            if branches.is_empty() {
                Expression::Unknown(text)
            } else {
                Expression::Branch(branches)
            }
        }
        "parenthesized_expression" | "cast_expression" => named_children(node)
            .last()
            .copied()
            .map(|child| expression_from_node(source, child))
            .unwrap_or(Expression::Unknown(text)),
        "binary_expression" | "update_expression" | "assignment_expression" => {
            Expression::Transformed(text)
        }
        _ => Expression::Unknown(text),
    }
}

fn extract_enum_domain(
    project: &JavaProject<'_>,
    enum_node: &GraphNode,
    accessor: &str,
) -> Result<Domain> {
    let source = project.source(&enum_node.file_path)?;
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .with_context(|| format!("parse enum source {}", enum_node.file_path))?;
    let declaration = descendants(tree.root_node())
        .into_iter()
        .find(|node| {
            node.kind() == "enum_declaration"
                && node.start_position().row + 1 == enum_node.start_line
        })
        .context("enum declaration not found")?;
    let body = declaration
        .child_by_field_name("body")
        .context("enum body not found")?;
    let fields = descendants(body)
        .into_iter()
        .filter(|child| child.kind() == "field_declaration")
        .filter(|child| !text_of(source, *child).contains(" static "))
        .filter_map(|field| {
            descendants(field)
                .into_iter()
                .find(|child| child.kind() == "variable_declarator")
                .and_then(|variable| variable.child_by_field_name("name"))
                .map(|name| text_of(source, name).to_owned())
        })
        .collect::<Vec<_>>();
    let signal = getter_signal(accessor).unwrap_or_else(|| accessor.to_owned());
    let Some(value_index) = fields.iter().position(|field| field == &signal) else {
        return Ok(incomplete_enum_domain(
            enum_node,
            accessor,
            "enum field projection missing",
        ));
    };
    let label_index = fields.iter().position(|field| {
        field != &signal
            && ["name", "desc", "label", "title"]
                .iter()
                .any(|suffix| field.to_ascii_lowercase().ends_with(suffix))
    });
    let constants = named_children(body)
        .into_iter()
        .filter(|child| child.kind() == "enum_constant")
        .collect::<Vec<_>>();
    let mut values = Vec::new();
    let mut complete = !constants.is_empty();
    for constant in constants {
        let name = constant
            .child_by_field_name("name")
            .map(|name| text_of(source, name).to_owned())
            .unwrap_or_default();
        let arguments = constant
            .child_by_field_name("arguments")
            .map(named_children)
            .unwrap_or_default();
        let value = arguments
            .get(value_index)
            .and_then(|node| wire_value(source, *node));
        let label = label_index
            .and_then(|index| arguments.get(index))
            .and_then(|node| string_literal(source, *node));
        let Some(value) = value else {
            complete = false;
            continue;
        };
        values.push(CodedValue {
            label: label.unwrap_or_else(|| render_wire_value(&value)),
            key: Some(name),
            value,
        });
    }
    let unique = values
        .iter()
        .map(|value| render_wire_value(&value.value))
        .collect::<BTreeSet<_>>()
        .len()
        == values.len();
    let mut domain = Domain {
        enum_fqn: Some(enum_node.qualified_name.replace("::", ".")),
        enum_source: Some(enum_node.file_path.clone()),
        accessor: Some(accessor.to_owned()),
        values,
        complete: complete && unique,
        ..Domain::default()
    };
    if !domain.complete {
        domain
            .unknown
            .insert("enum projection incomplete".to_owned());
    }
    Ok(domain)
}

fn linked_enum_nodes<'a>(project: &'a JavaProject<'_>, description: &str) -> Vec<&'a GraphNode> {
    let mut nodes = BTreeMap::new();
    let mut references = Vec::new();
    for (offset, _) in description.match_indices("{@link ") {
        let start = offset + "{@link ".len();
        let reference = description[start..]
            .split(|character: char| character.is_whitespace() || matches!(character, '}' | '#'))
            .next()
            .unwrap_or("")
            .trim();
        if reference.is_empty() {
            continue;
        }
        references.push(reference.to_owned());
    }
    references.extend(
        description
            .split(|character: char| {
                !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '$'))
            })
            .filter(|token| token.ends_with("Enum"))
            .map(ToOwned::to_owned),
    );
    references.sort();
    references.dedup();
    for reference in references {
        let node = project.node_for_fqn(&reference).or_else(|| {
            let simple = reference.rsplit('.').next()?;
            let candidates = project
                .graph()
                .candidates(simple)
                .into_iter()
                .filter(|node| node.kind == "enum")
                .collect::<Vec<_>>();
            (candidates.len() == 1).then(|| candidates[0])
        });
        if let Some(node) = node.filter(|node| node.kind == "enum") {
            nodes.insert(node.id.clone(), node);
        }
    }
    nodes.into_values().collect()
}

fn enum_accessor_candidates(field_name: &str) -> Vec<String> {
    let mut candidates = vec![format!("get{}", uppercase_first(field_name))];
    for (suffix, accessor) in [
        ("Type", "getType"),
        ("Status", "getStatus"),
        ("State", "getState"),
        ("Code", "getCode"),
        ("Id", "getId"),
    ] {
        if field_name.ends_with(suffix) && !candidates.iter().any(|value| value == accessor) {
            candidates.push(accessor.to_owned());
        }
    }
    for accessor in [
        "getCode",
        "getType",
        "getStatus",
        "getState",
        "getValue",
        "getId",
    ] {
        if !candidates.iter().any(|value| value == accessor) {
            candidates.push(accessor.to_owned());
        }
    }
    candidates
}

fn incomplete_enum_domain(enum_node: &GraphNode, accessor: &str, reason: &str) -> Domain {
    let mut domain = Domain {
        enum_fqn: Some(enum_node.qualified_name.replace("::", ".")),
        enum_source: Some(enum_node.file_path.clone()),
        accessor: Some(accessor.to_owned()),
        ..Domain::default()
    };
    domain.unknown.insert(reason.to_owned());
    domain
}

fn classify_patch(target: FieldTarget, domains: Vec<Domain>) -> SemanticPatch {
    let mut merged = Domain::default();
    for domain in &domains {
        merge_domain(&mut merged, domain.clone());
    }
    let identities = domains
        .iter()
        .filter_map(|domain| {
            Some((
                domain.enum_fqn.as_ref()?,
                domain.accessor.as_ref()?,
                domain.complete,
            ))
        })
        .collect::<BTreeSet<_>>();
    let all_closed = domains.iter().all(|domain| {
        domain.enum_fqn.is_some()
            && domain.complete
            && domain.external.is_empty()
            && domain.unknown.is_empty()
            && domain.literals.is_empty()
            && !domain.transformed
    });
    let status = if all_closed && identities.len() == 1 {
        ProvenanceStatus::Closed
    } else if merged.enum_fqn.is_some() {
        ProvenanceStatus::Known
    } else if domains.iter().all(|domain| {
        !domain.external.is_empty()
            && domain.unknown.is_empty()
            && domain.literals.is_empty()
            && !domain.transformed
    }) {
        ProvenanceStatus::External
    } else {
        ProvenanceStatus::Unresolved
    };
    let warning = match status {
        ProvenanceStatus::Closed => None,
        ProvenanceStatus::Known => {
            Some("enum evidence does not prove a complete field domain".to_owned())
        }
        ProvenanceStatus::External => Some("field value stops at an external boundary".to_owned()),
        ProvenanceStatus::Unresolved => Some(if merged.unknown.is_empty() {
            "field domain unresolved".to_owned()
        } else {
            format!(
                "field domain unresolved: {}",
                merged
                    .unknown
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }),
    };
    SemanticPatch {
        target,
        status,
        enum_fqn: merged.enum_fqn,
        enum_source: merged.enum_source,
        accessor: merged.accessor,
        values: if status == ProvenanceStatus::Closed {
            merged.values
        } else {
            Vec::new()
        },
        evidence: merged.evidence,
        warning,
    }
}

fn unresolved_patch(target: FieldTarget, reason: &str) -> SemanticPatch {
    SemanticPatch {
        target,
        status: ProvenanceStatus::Unresolved,
        enum_fqn: None,
        enum_source: None,
        accessor: None,
        values: Vec::new(),
        evidence: Vec::new(),
        warning: Some(reason.to_owned()),
    }
}

fn merge_domain(target: &mut Domain, source: Domain) {
    match (&target.enum_fqn, &source.enum_fqn) {
        (None, Some(_)) => {
            target.enum_fqn = source.enum_fqn.clone();
            target.enum_source = source.enum_source.clone();
            target.accessor = source.accessor.clone();
            target.values = source.values.clone();
            target.complete = source.complete;
        }
        (Some(left), Some(right)) if left != right || target.accessor != source.accessor => {
            target.complete = false;
            target
                .unknown
                .insert(format!("conflicting enum projections:{left}:{right}"));
        }
        _ => {}
    }
    target.external.extend(source.external);
    target.unknown.extend(source.unknown);
    target.literals.extend(source.literals);
    target.transformed |= source.transformed;
    for evidence in source.evidence {
        push_unique(&mut target.evidence, evidence);
    }
}

fn method_parameters(signature: &str) -> Vec<(String, String)> {
    let (Some(open), Some(close)) = (signature.find('('), signature.rfind(')')) else {
        return Vec::new();
    };
    split_top_level(&signature[open + 1..close])
        .into_iter()
        .filter_map(|parameter| {
            let parts = parameter
                .split_whitespace()
                .filter(|part| *part != "final" && !part.starts_with('@'))
                .collect::<Vec<_>>();
            (parts.len() >= 2).then(|| {
                (
                    parts[..parts.len() - 1].join(" "),
                    parts[parts.len() - 1].to_owned(),
                )
            })
        })
        .collect()
}

fn declared_variable_type(signature: &str, name: &str) -> Option<String> {
    let signature = signature.trim().trim_end_matches(';').trim();
    let position = signature.rfind(name)?;
    let suffix = signature[position + name.len()..].trim();
    if !suffix.is_empty() && !suffix.starts_with('=') {
        return None;
    }
    Some(signature[..position].trim().to_owned())
}

fn signature_arity(signature: &str) -> Option<usize> {
    let open = signature.find('(')?;
    let close = signature.rfind(')')?;
    let parameters = signature[open + 1..close].trim();
    if parameters.is_empty() {
        Some(0)
    } else {
        Some(split_top_level(parameters).len())
    }
}

fn split_top_level(value: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < value.len() {
        result.push(&value[start..]);
    }
    result
}

fn wire_value(source: &str, node: Node<'_>) -> Option<WireValue> {
    string_literal(source, node)
        .map(WireValue::String)
        .or_else(|| {
            text_of(source, node)
                .trim()
                .replace('_', "")
                .parse::<i64>()
                .ok()
                .map(WireValue::Number)
        })
}

fn string_literal(source: &str, node: Node<'_>) -> Option<String> {
    let value = text_of(source, node).trim();
    if !(value.starts_with('"') && value.ends_with('"')) {
        return None;
    }
    serde_json::from_str(value).ok()
}

fn render_wire_value(value: &WireValue) -> String {
    match value {
        WireValue::String(value) => value.clone(),
        WireValue::Number(value) => value.to_string(),
        WireValue::Decimal(value) => value.to_string(),
    }
}

fn render_path(graph: &Snapshot, reachable: &Reachability, target: &str) -> String {
    let mut path = vec![target.to_owned()];
    let mut current = target;
    while let Some(parent) = reachable.parent.get(current) {
        path.push(parent.clone());
        current = parent;
    }
    path.reverse();
    path.into_iter()
        .filter_map(|id| graph.nodes.get(&id))
        .map(|node| node.qualified_name.clone())
        .collect::<Vec<_>>()
        .join(" => ")
}

fn is_scalar(type_ref: &TypeRef) -> bool {
    matches!(
        type_ref.simple_name(),
        "String"
            | "CharSequence"
            | "char"
            | "Character"
            | "Integer"
            | "int"
            | "Short"
            | "short"
            | "Byte"
            | "byte"
            | "Long"
            | "long"
            | "BigInteger"
            | "Double"
            | "double"
            | "Float"
            | "float"
            | "BigDecimal"
            | "Date"
            | "LocalDate"
            | "LocalDateTime"
            | "Instant"
            | "Timestamp"
    )
}

fn is_collection(name: &str) -> bool {
    matches!(
        name,
        "List" | "Set" | "Collection" | "ArrayList" | "LinkedList" | "HashSet" | "Iterable"
    )
}

// Follow-up direction: continue internal RPC provenance through sibling CodeGraph indexes under a
// configured repositories root, such as ~/Documents/workspace/zhuanzhuan-rd. This run still stops
// at the current repository boundary and falls back to explicit DTO comment values.
fn is_external_path(path: &str) -> bool {
    let normalized = format!("/{path}");
    [
        "/infrastructure/thirdpart/",
        "/infrastructure/database/",
        "/repository/",
        "/mapper/",
        "/dao/",
        "/rpc/",
    ]
    .iter()
    .any(|segment| normalized.contains(segment))
}

fn is_service_path(path: &str) -> bool {
    let package_path = path.split("/java/").nth(1).unwrap_or(path);
    let normalized = format!("/{package_path}");
    normalized.contains("/service/")
        && !normalized.contains("/interfaces/")
        && !normalized.contains("/infrastructure/")
        && !normalized.contains("/repository/")
}

fn getter_signal(method_name: &str) -> Option<String> {
    method_name
        .strip_prefix("get")
        .or_else(|| method_name.strip_prefix("is"))
        .filter(|name| !name.is_empty())
        .map(lowercase_first)
}

fn join_path(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_owned()
    } else {
        format!("{prefix}.{field}")
    }
}

fn uppercase_first(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn lowercase_first(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn descendants(root: Node<'_>) -> Vec<Node<'_>> {
    let mut result = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        result.push(node);
        let mut children = named_children(node);
        children.reverse();
        stack.extend(children);
    }
    result
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn text_of<'a>(source: &'a str, node: Node<'_>) -> &'a str {
    &source[node.byte_range()]
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if values.len() < 20 && !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphEdge, GraphNode, test_snapshot};
    use crate::model::TargetIdentity;
    use std::fs;

    #[test]
    fn closed_requires_every_write_to_share_complete_enum() {
        let target = FieldTarget {
            operation_key: "Facade#query".to_owned(),
            schema_fqn: "p.DTO".to_owned(),
            field_path: "code".to_owned(),
            field_name: "code".to_owned(),
        };
        let domain = Domain {
            enum_fqn: Some("p.Action".to_owned()),
            enum_source: Some("p/Action.java".to_owned()),
            accessor: Some("getCode".to_owned()),
            values: vec![CodedValue {
                value: WireValue::String("a".to_owned()),
                key: Some("A".to_owned()),
                label: "A".to_owned(),
            }],
            complete: true,
            ..Domain::default()
        };
        assert_eq!(
            classify_patch(target.clone(), vec![domain.clone()]).status,
            ProvenanceStatus::Closed
        );
        let mut unknown = domain;
        unknown.unknown.insert("rpc".to_owned());
        assert_eq!(
            classify_patch(target, vec![unknown]).status,
            ProvenanceStatus::Known
        );
    }

    #[test]
    fn response_paths_keep_wrapper_and_array_segments() {
        let schemas = BTreeMap::from([(
            "p.Row".to_owned(),
            Schema {
                fqn: "p.Row".to_owned(),
                name: "Row".to_owned(),
                source_path: "Row.java".to_owned(),
                description: None,
                fields: vec![],
            },
        )]);
        let response = TypeRef {
            name: "PageList".to_owned(),
            arguments: vec![TypeRef {
                name: "p.Row".to_owned(),
                arguments: vec![],
                array_depth: 0,
            }],
            array_depth: 0,
        };
        assert_eq!(
            response_schema_paths(&response, &schemas),
            vec![("p.Row".to_owned(), "list[]".to_owned())]
        );
    }

    #[test]
    fn operation_chain_closes_complete_enum_without_name_guessing() {
        let repo = tempfile::tempdir().unwrap();
        write(
            repo.path(),
            "contract/src/main/java/p/contract/IFacade.java",
            "package p.contract;\nimport p.DTO;\n@ServiceContract\ninterface IFacade { ApiResult<DTO> query(QueryReq req); }\n",
        );
        write(
            repo.path(),
            "contract/src/main/java/p/DTO.java",
            "package p;\nclass DTO {\n  String code;\n  void setCode(String code) { this.code = code; }\n  static DTO of(String code) { DTO dto = new DTO(); dto.setCode(code); return dto; }\n}\n",
        );
        write(
            repo.path(),
            "contract/src/main/java/p/Action.java",
            "package p;\nenum Action {\n  A(\"a\", \"甲\"), B(\"b\", \"乙\");\n  final String code;\n  final String name;\n  Action(String code, String name) { this.code = code; this.name = name; }\n  String getCode() { return code; }\n}\n",
        );
        write(
            repo.path(),
            "service/src/main/java/p/interfaces/Facade.java",
            "package p.interfaces;\nimport p.service.Service;\nclass Facade {\n  Service service;\n  DTO query(QueryReq req) { return service.query(req); }\n}\n",
        );
        write(
            repo.path(),
            "service/src/main/java/p/service/Service.java",
            "package p.service;\nimport p.Action;\nimport p.DTO;\nclass Service {\n  DTO query(QueryReq req) { return java.util.stream.Stream.of(req).map(this::convert).findFirst().orElse(null); }\n  DTO convert(QueryReq req) { return build(Action.A); }\n  DTO build(Action action) { return DTO.of(action.getCode()); }\n}\n",
        );
        let graph = test_snapshot(
            vec![
                node(
                    "facade",
                    "interface",
                    "IFacade",
                    "p.contract::IFacade",
                    "contract/src/main/java/p/contract/IFacade.java",
                    4,
                    "",
                ),
                node(
                    "root",
                    "method",
                    "query",
                    "p.contract::IFacade::query",
                    "contract/src/main/java/p/contract/IFacade.java",
                    4,
                    "ApiResult<DTO> (QueryReq req)",
                ),
                node(
                    "impl-class",
                    "class",
                    "Facade",
                    "p.interfaces::Facade",
                    "service/src/main/java/p/interfaces/Facade.java",
                    3,
                    "",
                ),
                node(
                    "impl-field",
                    "field",
                    "service",
                    "p.interfaces::Facade::service",
                    "service/src/main/java/p/interfaces/Facade.java",
                    4,
                    "Service service",
                ),
                node(
                    "impl",
                    "method",
                    "query",
                    "p.interfaces::Facade::query",
                    "service/src/main/java/p/interfaces/Facade.java",
                    5,
                    "DTO (QueryReq req)",
                ),
                node(
                    "service-class",
                    "class",
                    "Service",
                    "p.service::Service",
                    "service/src/main/java/p/service/Service.java",
                    4,
                    "",
                ),
                node(
                    "service-query",
                    "method",
                    "query",
                    "p.service::Service::query",
                    "service/src/main/java/p/service/Service.java",
                    5,
                    "DTO (QueryReq req)",
                ),
                node(
                    "build",
                    "method",
                    "build",
                    "p.service::Service::build",
                    "service/src/main/java/p/service/Service.java",
                    7,
                    "DTO (Action action)",
                ),
                node(
                    "convert",
                    "method",
                    "convert",
                    "p.service::Service::convert",
                    "service/src/main/java/p/service/Service.java",
                    6,
                    "DTO (QueryReq req)",
                ),
                node(
                    "dto",
                    "class",
                    "DTO",
                    "p::DTO",
                    "contract/src/main/java/p/DTO.java",
                    2,
                    "",
                ),
                node(
                    "field",
                    "field",
                    "code",
                    "p::DTO::code",
                    "contract/src/main/java/p/DTO.java",
                    3,
                    "String code",
                ),
                node(
                    "setter",
                    "method",
                    "setCode",
                    "p::DTO::setCode",
                    "contract/src/main/java/p/DTO.java",
                    4,
                    "void (String code)",
                ),
                node(
                    "of",
                    "method",
                    "of",
                    "p::DTO::of",
                    "contract/src/main/java/p/DTO.java",
                    5,
                    "DTO (String code)",
                ),
                node(
                    "enum",
                    "enum",
                    "Action",
                    "p::Action",
                    "contract/src/main/java/p/Action.java",
                    2,
                    "",
                ),
            ],
            vec![
                contains("facade", "root"),
                contains("impl-class", "impl-field"),
                contains("impl-class", "impl"),
                contains("service-class", "service-query"),
                contains("service-class", "convert"),
                contains("service-class", "build"),
                contains("dto", "field"),
                contains("dto", "setter"),
                contains("dto", "of"),
                call("root", "impl", 4),
                call("of", "setter", 5),
            ],
        );
        let project = JavaProject::load(repo.path(), &graph).unwrap();
        let identity = TargetIdentity {
            app_name: "demo".to_owned(),
            branch: "feature".to_owned(),
            commit: "deadbeef".to_owned(),
            codegraph_version: "test".to_owned(),
            codegraph_extraction_version: "test".to_owned(),
        };
        let (mut operations, schemas) = project
            .build_contracts(&identity, &["contract/src/main/java/p".to_owned()])
            .unwrap();
        SemanticAnalyzer::new(&project)
            .enrich(&mut operations, &schemas)
            .unwrap();

        let operation = &operations[0];
        assert_eq!(operation.service.as_ref().unwrap().class_name, "Service");
        let patch = operation
            .semantic_patches
            .iter()
            .find(|patch| patch.target.field_name == "code")
            .unwrap();
        assert_eq!(patch.status, ProvenanceStatus::Closed);
        assert_eq!(
            patch
                .values
                .iter()
                .map(|value| &value.value)
                .collect::<Vec<_>>(),
            vec![
                &WireValue::String("a".to_owned()),
                &WireValue::String("b".to_owned())
            ]
        );
    }

    fn write(root: &std::path::Path, relative: &str, source: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn node(
        id: &str,
        kind: &str,
        name: &str,
        qualified_name: &str,
        file_path: &str,
        line: usize,
        signature: &str,
    ) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            file_path: file_path.to_owned(),
            start_line: line,
            start_column: 0,
            docstring: None,
            signature: signature.to_owned(),
            decorators: String::new(),
            return_type: String::new(),
        }
    }

    fn contains(source: &str, target: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_owned(),
            target: target.to_owned(),
            kind: "contains".to_owned(),
            line: 0,
            column: 0,
            metadata: String::new(),
            provenance: String::new(),
        }
    }

    fn call(source: &str, target: &str, line: usize) -> GraphEdge {
        GraphEdge {
            source: source.to_owned(),
            target: target.to_owned(),
            kind: "calls".to_owned(),
            line,
            column: 0,
            metadata: String::new(),
            provenance: String::new(),
        }
    }
}
