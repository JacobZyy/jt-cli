use super::lookup::{ancestors, method_declaration, statements, unwrap_parentheses};
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestOrigin {
    java_type: TypeRef,
    path: String,
    guaranteed: bool,
}

impl SemanticAnalyzer<'_> {
    pub(super) fn request_patches(
        &mut self,
        operation: &Operation,
        schemas: &BTreeMap<String, Schema>,
        reachable: &Reachability,
    ) -> Result<Vec<SemanticPatch>> {
        let Some(request) = &operation.request else {
            return Ok(Vec::new());
        };
        let mut sites = BTreeMap::<String, Vec<(Domain, bool)>>::new();
        let mut methods = reachable
            .nodes
            .iter()
            .filter_map(|id| self.project.graph().nodes.get(id))
            .cloned()
            .collect::<Vec<_>>();
        methods.sort_by(|left, right| left.id.cmp(&right.id));
        for method in methods {
            for invocation in self.method_invocations(&method)? {
                if invocation.arguments.len() != 1 {
                    continue;
                }
                let targets = self.resolve_invocation(&method, &invocation)?;
                let Some(lookup) = targets
                    .first()
                    .filter(|_| targets.len() == 1)
                    .and_then(|id| self.enum_lookups.get(id))
                    .cloned()
                else {
                    continue;
                };
                let Some(origin) = self.request_origin(
                    operation,
                    &method,
                    invocation.arguments[0].clone(),
                    invocation.offset,
                    reachable,
                    &mut BTreeSet::new(),
                )?
                else {
                    continue;
                };
                if origin.path.is_empty() {
                    continue;
                }
                let validated = lookup.rejects_unknown
                    && origin.guaranteed
                    && self.lookup_is_guarded(&method, invocation.offset)?;
                let mut domain = lookup.domain;
                push_unique(
                    &mut domain.evidence,
                    format!(
                        "lookup:{}:{}:{}:{}({})",
                        method.file_path,
                        invocation.line,
                        invocation.column,
                        invocation.name,
                        origin.path,
                    ),
                );
                if validated {
                    push_unique(
                        &mut domain.evidence,
                        format!("rejects-unmatched:{}:{}", method.file_path, invocation.line),
                    );
                }
                push_unique(
                    &mut domain.evidence,
                    format!(
                        "chain:{}",
                        render_path(self.project.graph(), reachable, &method.id)
                    ),
                );
                sites
                    .entry(origin.path)
                    .or_default()
                    .push((domain, validated));
            }
        }
        let mut patches = Vec::new();
        for (schema_fqn, prefix) in response_schema_paths(request, schemas) {
            for field in &schemas[&schema_fqn].fields {
                if !is_scalar(&field.java_type) {
                    continue;
                }
                let path = join_path(&prefix, &field.name);
                let target = FieldTarget {
                    source: FieldSource::Request,
                    operation_key: operation.key.clone(),
                    schema_fqn: schema_fqn.clone(),
                    field_path: path.clone(),
                    field_name: field.name.clone(),
                };
                let Some(candidates) = sites.get(&path) else {
                    patches.push(unresolved_patch(
                        target,
                        "no operation-reachable enum lookup",
                    ));
                    continue;
                };
                let identities = candidates
                    .iter()
                    .map(|(domain, _)| (&domain.enum_fqn, &domain.accessor))
                    .collect::<BTreeSet<_>>();
                let validated =
                    identities.len() == 1 && candidates.iter().any(|(_, validated)| *validated);
                let mut domain = Domain::default();
                for (candidate, _) in candidates {
                    merge_domain(&mut domain, candidate.clone());
                }
                if !validated {
                    domain.unknown.insert(
                        "enum lookup does not prove rejection of unmatched request values"
                            .to_owned(),
                    );
                }
                self.request_domains
                    .insert((operation.key.clone(), path), domain.clone());
                patches.push(classify_patch(target, vec![domain]));
            }
        }
        Ok(patches)
    }

    pub(super) fn request_expression_domain(
        &mut self,
        operation: &Operation,
        method: &GraphNode,
        expression: Expression,
        offset: usize,
        reachable: &Reachability,
    ) -> Result<Option<Domain>> {
        let Some(origin) = self.request_origin(
            operation,
            method,
            expression,
            offset,
            reachable,
            &mut BTreeSet::new(),
        )?
        else {
            return Ok(None);
        };
        Ok(self
            .request_domains
            .get(&(operation.key.clone(), origin.path))
            .cloned())
    }

    fn request_origin(
        &mut self,
        operation: &Operation,
        method: &GraphNode,
        expression: Expression,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<String>,
    ) -> Result<Option<RequestOrigin>> {
        let key = format!("{}:{offset}:{expression:?}", method.id);
        if visiting.len() >= 32 || !visiting.insert(key.clone()) {
            return Ok(None);
        }
        let result =
            self.request_origin_inner(operation, method, expression, offset, reachable, visiting);
        visiting.remove(&key);
        result
    }

    fn request_origin_inner(
        &mut self,
        operation: &Operation,
        method: &GraphNode,
        expression: Expression,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<String>,
    ) -> Result<Option<RequestOrigin>> {
        match expression {
            Expression::Identifier(name) => {
                if let Some((value, declaration_offset)) =
                    self.local_value(method, &name, offset)?
                {
                    return self.request_origin(
                        operation,
                        method,
                        value,
                        declaration_offset,
                        reachable,
                        visiting,
                    );
                }
                let parsed = self.parsed(&method.file_path)?;
                if method_declaration(parsed, method).is_some_and(|declaration| {
                    descendants(declaration).iter().any(|node| {
                        node.start_byte() < offset
                            && node.kind() == "assignment_expression"
                            && node
                                .child_by_field_name("left")
                                .is_some_and(|left| text_of(&parsed.source, left) == name)
                    })
                }) {
                    return Ok(None);
                }
                let parameters = method_parameters(&method.signature);
                let Some(index) = parameters
                    .iter()
                    .position(|(_, parameter)| parameter == &name)
                else {
                    let parsed = self.parsed(&method.file_path)?;
                    let outer_line = method_declaration(parsed, method).and_then(|declaration| {
                        ancestors(declaration)
                            .find(|node| node.kind() == "method_declaration")
                            .map(|outer| outer.start_position().row + 1)
                    });
                    let outer = outer_line
                        .and_then(|line| {
                            self.project.graph().nodes.values().find(|node| {
                                node.kind == "method"
                                    && node.file_path == method.file_path
                                    && node.start_line == line
                            })
                        })
                        .cloned();
                    return if let Some(outer) = outer {
                        self.request_origin(
                            operation,
                            &outer,
                            Expression::Identifier(name),
                            offset,
                            reachable,
                            visiting,
                        )
                    } else {
                        Ok(None)
                    };
                };
                let root = operation_root(self.project.graph(), operation)?;
                if method.id == root.id {
                    let Some(request) = &operation.request else {
                        return Ok(None);
                    };
                    let owner = method
                        .qualified_name
                        .rsplit_once("::")
                        .map(|(owner, _)| owner)
                        .unwrap_or("");
                    let parameter = parse_java_type(&parameters[index].0)
                        .and_then(|ty| self.project.resolve_type(&method.file_path, owner, &ty));
                    return Ok(parameter
                        .filter(|node| node.qualified_name.replace("::", ".") == request.name)
                        .map(|_| RequestOrigin {
                            java_type: request.clone(),
                            path: String::new(),
                            guaranteed: true,
                        }));
                }
                // Interface dispatch has no Java invocation expression at the contract declaration.
                if method.name == root.name
                    && parameters.len() == method_parameters(&root.signature).len()
                    && self
                        .project
                        .graph()
                        .incoming_calls(&method.id)
                        .any(|edge| edge.source == root.id)
                {
                    let root = root.clone();
                    let root_name = method_parameters(&root.signature)[index].1.clone();
                    return self.request_origin(
                        operation,
                        &root,
                        Expression::Identifier(root_name),
                        offset,
                        reachable,
                        visiting,
                    );
                }
                let callers = reachable
                    .nodes
                    .iter()
                    .filter_map(|id| self.project.graph().nodes.get(id))
                    .filter(|caller| caller.id != method.id)
                    .cloned()
                    .collect::<Vec<_>>();
                let mut origins = Vec::new();
                for caller in callers {
                    for invocation in self.method_invocations(&caller)? {
                        if invocation.arguments.len() <= index
                            || !self
                                .resolve_invocation(&caller, &invocation)?
                                .contains(&method.id)
                        {
                            continue;
                        }
                        let Some(mut origin) = self.request_origin(
                            operation,
                            &caller,
                            invocation.arguments[index].clone(),
                            invocation.offset,
                            reachable,
                            visiting,
                        )?
                        else {
                            return Ok(None);
                        };
                        origin.guaranteed &=
                            self.site_is_unconditional(&caller, invocation.offset)?;
                        origins.push(origin);
                    }
                }
                let Some(first) = origins.first().cloned() else {
                    return Ok(None);
                };
                Ok(origins
                    .iter()
                    .all(|origin| origin.path == first.path && origin.java_type == first.java_type)
                    .then(|| RequestOrigin {
                        guaranteed: origins.iter().all(|origin| origin.guaranteed),
                        ..first
                    }))
            }
            Expression::Getter { receiver, accessor } => {
                let Some(mut origin) = self.request_origin(
                    operation,
                    method,
                    receiver_expression(&receiver),
                    offset,
                    reachable,
                    visiting,
                )?
                else {
                    return Ok(None);
                };
                let Some(field_name) = getter_signal(&accessor) else {
                    return Ok(None);
                };
                let setter = format!("set{}", uppercase_first(&field_name));
                if self.method_invocations(method)?.iter().any(|site| {
                    site.offset < offset
                        && site.name == setter
                        && site.receiver.as_deref() == Some(receiver.as_str())
                }) {
                    return Ok(None);
                }
                let parsed = self.parsed(&method.file_path)?;
                if method_declaration(parsed, method).is_some_and(|declaration| {
                    descendants(declaration).iter().any(|node| {
                        node.start_byte() < offset
                            && node.kind() == "assignment_expression"
                            && node.child_by_field_name("left").is_some_and(|left| {
                                text_of(&parsed.source, left) == format!("{receiver}.{field_name}")
                            })
                    })
                }) {
                    return Ok(None);
                }
                let Some(class) = self.project.node_for_fqn(&origin.java_type.name) else {
                    return Ok(None);
                };
                let Some(field) = self
                    .project
                    .graph()
                    .contained(&class.id, "field")
                    .into_iter()
                    .find(|field| field.name == field_name)
                else {
                    return Ok(None);
                };
                let Some(mut ty) = declared_variable_type(&field.signature, &field.name)
                    .and_then(|ty| parse_java_type(&ty))
                else {
                    return Ok(None);
                };
                if let Some(node) =
                    self.project
                        .resolve_type(&field.file_path, &class.qualified_name, &ty)
                {
                    ty.name = node.qualified_name.replace("::", ".");
                }
                origin.path = join_path(&origin.path, &field_name);
                origin.java_type = ty;
                Ok(Some(origin))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn local_value(
        &mut self,
        method: &GraphNode,
        name: &str,
        offset: usize,
    ) -> Result<Option<(Expression, usize)>> {
        let parsed = self.parsed(&method.file_path)?;
        let Some(declaration) = method_declaration(parsed, method) else {
            return Ok(None);
        };
        let nodes = descendants(declaration);
        let variable = nodes
            .iter()
            .filter(|node| node.kind() == "variable_declarator" && node.start_byte() < offset)
            .filter(|node| {
                node.child_by_field_name("name")
                    .is_some_and(|node| text_of(&parsed.source, node) == name)
            })
            .filter(|node| {
                ancestors(**node)
                    .find(|node| matches!(node.kind(), "block" | "constructor_body"))
                    .is_some_and(|block| block.start_byte() <= offset && offset < block.end_byte())
            })
            .max_by_key(|node| node.start_byte())
            .copied();
        let Some(variable) = variable else {
            return Ok(None);
        };
        if nodes.iter().any(|node| {
            node.start_byte() > variable.start_byte()
                && node.start_byte() < offset
                && ((node.kind() == "assignment_expression"
                    && node
                        .child_by_field_name("left")
                        .is_some_and(|left| text_of(&parsed.source, left) == name))
                    || (node.kind() == "update_expression"
                        && named_children(*node)
                            .iter()
                            .any(|child| text_of(&parsed.source, *child) == name)))
        }) {
            return Ok(Some((
                Expression::Unknown(format!("reassigned local:{name}")),
                variable.start_byte(),
            )));
        }
        Ok(variable.child_by_field_name("value").map(|value| {
            (
                expression_from_node(&parsed.source, value),
                value.start_byte(),
            )
        }))
    }

    fn site_is_unconditional(&mut self, method: &GraphNode, offset: usize) -> Result<bool> {
        let parsed = self.parsed(&method.file_path)?;
        Ok(invocation_at(parsed, offset).is_some_and(unconditional))
    }

    fn lookup_is_guarded(&mut self, method: &GraphNode, offset: usize) -> Result<bool> {
        let parsed = self.parsed(&method.file_path)?;
        let Some(invocation) = invocation_at(parsed, offset) else {
            return Ok(false);
        };
        if !unconditional(invocation) {
            return Ok(false);
        }
        if let Some(parent) = invocation.parent().and_then(|node| node.parent()) {
            if nonnull_guard(&parsed.source, parent, text_of(&parsed.source, invocation)) {
                return Ok(true);
            }
        }
        let Some(variable) = invocation
            .parent()
            .filter(|node| node.kind() == "variable_declarator")
        else {
            return Ok(false);
        };
        let name = variable
            .child_by_field_name("name")
            .map(|name| text_of(&parsed.source, name))
            .unwrap_or("");
        let Some(statement) = variable.parent() else {
            return Ok(false);
        };
        let Some(block) = statement.parent().filter(|node| node.kind() == "block") else {
            return Ok(false);
        };
        if let Some(next) = statements(block)
            .into_iter()
            .find(|next| next.start_byte() > statement.start_byte())
        {
            if next.kind() == "expression_statement"
                && next
                    .named_child(0)
                    .is_some_and(|node| nonnull_guard(&parsed.source, node, name))
            {
                return Ok(true);
            }
            if next.kind() == "if_statement" {
                let condition = next
                    .child_by_field_name("condition")
                    .map(unwrap_parentheses);
                let rejects = next.child_by_field_name("consequence").is_some_and(|node| {
                    node.kind() == "throw_statement"
                        || (node.kind() == "block"
                            && statements(node)
                                .first()
                                .is_some_and(|node| node.kind() == "throw_statement"))
                });
                if rejects && condition.is_some_and(|node| null_check(&parsed.source, node, name)) {
                    return Ok(true);
                }
            }
            // Do not cross intervening effects or branches to claim a mandatory guard.
            return Ok(false);
        }
        Ok(false)
    }
}

fn invocation_at(parsed: &ParsedFile, offset: usize) -> Option<Node<'_>> {
    descendants(parsed.tree.root_node())
        .into_iter()
        .find(|node| node.kind() == "method_invocation" && node.start_byte() == offset)
}

fn unconditional(node: Node<'_>) -> bool {
    if ancestors(node).any(|node| {
        matches!(
            node.kind(),
            "if_statement"
                | "ternary_expression"
                | "switch_expression"
                | "switch_statement"
                | "for_statement"
                | "enhanced_for_statement"
                | "while_statement"
                | "do_statement"
                | "lambda_expression"
                | "try_statement"
                | "catch_clause"
                | "binary_expression"
        )
    }) {
        return false;
    }
    let mut child = node;
    for parent in ancestors(node) {
        if parent.kind() == "block" {
            let owner = ancestors(parent).find(|node| node.kind() == "method_declaration");
            let bypass = statements(parent)
                .into_iter()
                .filter(|statement| statement.end_byte() <= child.start_byte())
                .any(|statement| {
                    descendants(statement).iter().any(|node| {
                        node.kind() == "return_statement"
                            && ancestors(*node).find(|node| node.kind() == "method_declaration")
                                == owner
                    })
                });
            if bypass {
                return false;
            }
        }
        child = parent;
    }
    true
}

fn nonnull_guard(source: &str, node: Node<'_>, value: &str) -> bool {
    if node.kind() != "method_invocation" {
        return false;
    }
    let method = node
        .child_by_field_name("name")
        .map(|name| text_of(source, name))
        .unwrap_or("");
    let receiver = node
        .child_by_field_name("object")
        .map(|object| text_of(source, object))
        .unwrap_or("");
    let known = matches!(
        (receiver, method),
        ("ZzAssert" | "Assert", "notNull")
            | ("Objects" | "java.util.Objects", "requireNonNull")
            | ("Preconditions", "checkNotNull")
    );
    known
        && node
            .child_by_field_name("arguments")
            .and_then(|args| args.named_child(0))
            .is_some_and(|arg| text_of(source, arg) == value)
}

fn null_check(source: &str, node: Node<'_>, value: &str) -> bool {
    if node.kind() == "binary_expression" {
        let left = node
            .child_by_field_name("left")
            .map(|node| text_of(source, node))
            .unwrap_or("");
        let right = node
            .child_by_field_name("right")
            .map(|node| text_of(source, node))
            .unwrap_or("");
        return node
            .child_by_field_name("operator")
            .is_some_and(|op| text_of(source, op) == "==")
            && ((left == value && right == "null") || (right == value && left == "null"));
    }
    node.kind() == "method_invocation"
        && node.child_by_field_name("object").is_some_and(|object| {
            matches!(text_of(source, object), "Objects" | "java.util.Objects")
        })
        && node
            .child_by_field_name("name")
            .is_some_and(|name| text_of(source, name) == "isNull")
        && node
            .child_by_field_name("arguments")
            .and_then(|args| args.named_child(0))
            .is_some_and(|arg| text_of(source, arg) == value)
}

fn receiver_expression(receiver: &str) -> Expression {
    if let Some(call) = receiver.strip_suffix("()") {
        if let Some((object, accessor)) = call.rsplit_once('.') {
            if getter_signal(accessor).is_some() {
                return Expression::Getter {
                    receiver: object.to_owned(),
                    accessor: accessor.to_owned(),
                };
            }
        }
        return Expression::Unknown(receiver.to_owned());
    }
    Expression::Identifier(receiver.to_owned())
}
