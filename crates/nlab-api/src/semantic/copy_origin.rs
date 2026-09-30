use super::*;

use std::collections::BTreeSet;

const KNOWN_RESULT_WRAPPERS: &[&str] = &["ApiResult", "Result", "Response"];
const KNOWN_RESULT_FACTORIES: &[&str] = &["success", "returnSuccess", "ok", "of"];
const MAX_COPY_ORIGIN_DEPTH: usize = 48;

type SetterOrigin = (String, String);

#[derive(Clone)]
struct ReturnValue {
    expression: Expression,
    offset: usize,
    wrapper_arguments: Option<Vec<(Expression, usize)>>,
}

impl SemanticAnalyzer<'_> {
    /// Return setter receivers whose object identity is proven to be `receiver`.
    ///
    /// Empty means the flow could not be proven. It deliberately does not fall back
    /// to same-type setter calls: a DTO class can have several live instances.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn copied_field_origins(
        &mut self,
        _operation: &Operation,
        writer: &GraphNode,
        receiver: &str,
        offset: usize,
        reachable: &Reachability,
    ) -> Result<BTreeSet<SetterOrigin>> {
        let Some(origins) =
            self.trace_receiver(writer, receiver, offset, reachable, &mut BTreeSet::new(), 0)?
        else {
            return Ok(BTreeSet::new());
        };
        Ok(origins)
    }

    /// Check one indexed setter edge against the getter receiver's proven origins.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn copied_setter_edge_matches(
        &mut self,
        operation: &Operation,
        writer: &GraphNode,
        receiver: &str,
        offset: usize,
        reachable: &Reachability,
        edge: &GraphEdge,
        setter_name: &str,
    ) -> Result<bool> {
        if !reachable.nodes.contains(&edge.source) {
            return Ok(false);
        }
        let origins = self.copied_field_origins(operation, writer, receiver, offset, reachable)?;
        let Some(source) = self.project.graph().nodes.get(&edge.source).cloned() else {
            return Ok(false);
        };
        let Some((setter_receiver, setter_offset)) =
            self.setter_receiver(edge, &source, setter_name)?
        else {
            return Ok(false);
        };
        let assigned = self.trace_receiver(
            &source,
            &setter_receiver,
            setter_offset,
            reachable,
            &mut BTreeSet::new(),
            0,
        )?;
        Ok(assigned.is_some_and(|assigned| !assigned.is_disjoint(&origins)))
    }

    // Permit definite assignment through a try whose catch paths all throw.
    fn copy_local_value(
        &mut self,
        method: &GraphNode,
        name: &str,
        offset: usize,
    ) -> Result<Option<(Expression, usize)>> {
        let value = self.local_value(method, name, offset)?;
        if !matches!(&value, Some((Expression::Unknown(reason), _)) if reason.starts_with("reassigned local:"))
        {
            return Ok(value);
        }
        let parsed = self.parsed(&method.file_path)?;
        let Some(declaration) = lookup::method_declaration(parsed, method) else {
            return Ok(value);
        };
        let nodes = descendants(declaration);
        let Some(variable) = nodes
            .iter()
            .filter(|node| node.kind() == "variable_declarator" && node.start_byte() < offset)
            .filter(|node| {
                node.child_by_field_name("name")
                    .is_some_and(|node| text_of(&parsed.source, node) == name)
            })
            .max_by_key(|node| node.start_byte())
        else {
            return Ok(value);
        };
        if variable.child_by_field_name("value").is_some()
            && !variable
                .child_by_field_name("value")
                .is_some_and(|node| text_of(&parsed.source, node) == "null")
        {
            return Ok(value);
        }
        let assignments = nodes
            .iter()
            .filter(|node| {
                node.kind() == "assignment_expression"
                    && node.start_byte() > variable.start_byte()
                    && node.start_byte() < offset
            })
            .filter(|node| {
                node.child_by_field_name("left")
                    .is_some_and(|left| text_of(&parsed.source, left) == name)
            })
            .collect::<Vec<_>>();
        let [assignment] = assignments.as_slice() else {
            return Ok(value);
        };
        if assignment
            .child_by_field_name("operator")
            .is_none_or(|node| text_of(&parsed.source, node) != "=")
        {
            return Ok(value);
        }
        let nullable = variable.child_by_field_name("value").is_some();
        for parent in lookup::ancestors(**assignment).take_while(|node| *node != declaration) {
            if matches!(
                parent.kind(),
                "switch_expression"
                    | "switch_statement"
                    | "for_statement"
                    | "enhanced_for_statement"
                    | "while_statement"
                    | "do_statement"
                    | "catch_clause"
                    | "finally_clause"
            ) {
                return Ok(value);
            }
            if parent.kind() == "if_statement" && !nullable {
                return Ok(value);
            }
            if parent.kind() == "try_statement" {
                for catch in named_children(parent)
                    .into_iter()
                    .filter(|node| node.kind() == "catch_clause")
                {
                    if !catch.child_by_field_name("body").is_some_and(|body| {
                        lookup::statements(body)
                            .last()
                            .is_some_and(|node| node.kind() == "throw_statement")
                    }) {
                        return Ok(value);
                    }
                }
            }
        }
        Ok(assignment.child_by_field_name("right").map(|right| {
            (
                if nullable {
                    Expression::Branch(vec![
                        Expression::Literal("null".to_owned()),
                        expression_from_node(&parsed.source, right),
                    ])
                } else {
                    expression_from_node(&parsed.source, right)
                },
                right.start_byte(),
            )
        }))
    }

    fn trace_receiver(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        let receiver = receiver.trim();
        if receiver.is_empty() || depth >= MAX_COPY_ORIGIN_DEPTH {
            return Ok(None);
        }
        if receiver.contains('(') && !receiver.ends_with(".getData()") {
            let parsed = self.parsed(&method.file_path)?.clone();
            if let Some(node) = values::expression_node(&parsed, method, receiver, offset) {
                return self.trace_expression(
                    method,
                    expression_from_node(&parsed.source, node),
                    node.start_byte(),
                    reachable,
                    visiting,
                    depth + 1,
                );
            }
        }
        if let Some((base, accessor)) = receiver.rsplit_once(".") {
            if accessor.ends_with("()") {
                let accessor = accessor.trim_end_matches("()");
                if accessor == "getData" {
                    if !self.is_known_result_receiver(method, base, offset)? {
                        return Ok(None);
                    }
                    return self.trace_wrapper_receiver(
                        method,
                        base,
                        offset,
                        reachable,
                        visiting,
                        depth + 1,
                    );
                }
                if getter_signal(accessor).is_some() {
                    return self.trace_holder_getter(
                        method,
                        base,
                        accessor,
                        offset,
                        reachable,
                        visiting,
                        depth + 1,
                    );
                }
            }
        }
        self.trace_expression(
            method,
            Expression::Identifier(normalize_receiver(receiver)),
            offset,
            reachable,
            visiting,
            depth,
        )
    }

    fn trace_expression(
        &mut self,
        method: &GraphNode,
        expression: Expression,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        if depth >= MAX_COPY_ORIGIN_DEPTH {
            return Ok(None);
        }
        match expression {
            // A null object contributes no field value or setter origin.
            Expression::Literal(value) if value == "null" => Ok(Some(BTreeSet::new())),
            Expression::Identifier(name) => {
                let name = normalize_receiver(&name);
                let parsed = self.parsed(&method.file_path)?.clone();
                if let Some(declaration) = lookup::method_declaration(&parsed, method)
                    && let Some(loop_node) = descendants(declaration).into_iter().find(|node| {
                        node.kind() == "enhanced_for_statement"
                            && node
                                .child_by_field_name("name")
                                .is_some_and(|item| text_of(&parsed.source, item) == name)
                            && node.child_by_field_name("body").is_some_and(|body| {
                                body.start_byte() <= offset && offset <= body.end_byte()
                            })
                    })
                    && let Some(collection) = loop_node.child_by_field_name("value")
                {
                    if descendants(loop_node).iter().any(|node| {
                        node.start_byte() < offset
                            && matches!(node.kind(), "assignment_expression" | "update_expression")
                            && node
                                .child_by_field_name("left")
                                .or_else(|| node.named_child(0))
                                .is_some_and(|left| text_of(&parsed.source, left) == name)
                    }) {
                        return Ok(None);
                    }
                    return self.trace_expression(
                        method,
                        expression_from_node(&parsed.source, collection),
                        collection.start_byte(),
                        reachable,
                        visiting,
                        depth + 1,
                    );
                }
                if let Some(index) = method_parameters(&method.signature)
                    .iter()
                    .position(|(_, parameter)| parameter == &name)
                {
                    return self.trace_parameter(method, index, reachable, visiting, depth + 1);
                }
                if let Some((value, declaration_offset)) =
                    self.copy_local_value(method, &name, offset)?
                {
                    if declaration_offset >= offset {
                        return Ok(None);
                    }
                    if matches!(&value, Expression::Unknown(value) if value.trim_start().starts_with("new "))
                    {
                        if self.copy_collection_type(method, &name, offset)?.is_some() {
                            if let Some(declaration) = lookup::method_declaration(&parsed, method)
                                && descendants(declaration).iter().any(|node| {
                                    node.start_byte() > declaration_offset
                                        && node.start_byte() < offset
                                        && matches!(
                                            node.kind(),
                                            "variable_declarator" | "assignment_expression"
                                        )
                                        && node
                                            .child_by_field_name("value")
                                            .or_else(|| node.child_by_field_name("right"))
                                            .is_some_and(|value| {
                                                text_of(&parsed.source, value) == name
                                            })
                                })
                            {
                                return Ok(None);
                            }
                            let Some(mut origins) = self.trace_expression(
                                method,
                                value.clone(),
                                declaration_offset,
                                reachable,
                                visiting,
                                depth + 1,
                            )?
                            else {
                                return Ok(None);
                            };
                            for site in self.method_invocations(method)? {
                                if site.offset > declaration_offset && site.offset < offset
                                    && site.arguments.iter().any(|argument| matches!(argument, Expression::Identifier(value) if value == &name))
                                    && !self.copy_collection_escape_is_setter(method, &site, &name)?
                                { return Ok(None); }
                                if site.offset <= declaration_offset
                                    || site.offset >= offset
                                    || site.receiver.as_deref() != Some(&name)
                                {
                                    continue;
                                }
                                let index = match site.name.as_str() {
                                    "add" | "addAll" | "putAll" => 0,
                                    "put" | "set" => 1,
                                    "isEmpty" | "size" | "containsKey" | "get" | "remove"
                                    | "clear" => continue,
                                    _ => return Ok(None),
                                };
                                let Some((value, offset)) =
                                    self.invocation_argument(method, &site, index)?
                                else {
                                    return Ok(None);
                                };
                                let Some(value) = self.trace_expression(
                                    method,
                                    value,
                                    offset,
                                    reachable,
                                    visiting,
                                    depth + 1,
                                )?
                                else {
                                    return Ok(None);
                                };
                                origins.extend(value);
                            }
                            return Ok(Some(origins));
                        }
                        return Ok(Some(BTreeSet::from([(
                            method.id.clone(),
                            format!("{name}@{declaration_offset}"),
                        )])));
                    }
                    return self.trace_expression(
                        method,
                        value,
                        declaration_offset,
                        reachable,
                        visiting,
                        depth + 1,
                    );
                }
                if self.is_local_variable(method, &name, offset)? {
                    return Ok(Some(BTreeSet::from([(method.id.clone(), name)])));
                }
                Ok(None)
            }
            Expression::Getter { receiver, accessor } => {
                if accessor == "getData" {
                    if !self.is_known_result_receiver(method, &receiver, offset)? {
                        return Ok(None);
                    }
                    return self.trace_wrapper_receiver(
                        method,
                        &receiver,
                        offset,
                        reachable,
                        visiting,
                        depth + 1,
                    );
                }
                self.trace_holder_getter(
                    method,
                    &receiver,
                    &accessor,
                    offset,
                    reachable,
                    visiting,
                    depth + 1,
                )
            }
            Expression::Unknown(source) if source.trim_start().starts_with("new ") => {
                let parsed = self.parsed(&method.file_path)?.clone();
                let Some(node) = values::expression_node(&parsed, method, &source, offset) else {
                    return Ok(None);
                };
                let Some(kind) = node.child_by_field_name("type").and_then(|kind| {
                    parse_java_type(text_of(&parsed.source, kind).trim_end_matches("<>"))
                }) else {
                    return Ok(None);
                };
                if !self
                    .project
                    .imported_type(&method.file_path, &kind.name)
                    .is_some_and(|name| {
                        matches!(
                            name.as_str(),
                            "java.util.ArrayList" | "java.util.HashMap" | "java.util.LinkedHashMap"
                        )
                    })
                    || self
                        .project
                        .resolve_type(&method.file_path, &method.qualified_name, &kind)
                        .is_some()
                {
                    return Ok(None);
                }
                let args = node
                    .child_by_field_name("arguments")
                    .map(named_children)
                    .unwrap_or_default();
                match args.as_slice() {
                    [] => Ok(Some(BTreeSet::new())),
                    [argument] if text_of(&parsed.source, *argument).parse::<usize>().is_ok() => {
                        Ok(Some(BTreeSet::new()))
                    }
                    [argument] => self.trace_expression(
                        method,
                        expression_from_node(&parsed.source, *argument),
                        argument.start_byte(),
                        reachable,
                        visiting,
                        depth + 1,
                    ),
                    _ => Ok(None),
                }
            }
            Expression::Call { name, source } => {
                let parsed = self.parsed(&method.file_path)?.clone();
                if let Some(node) = values::expression_node(&parsed, method, &source, offset)
                    && self.copy_pipeline(method, node, &parsed.source)?
                {
                    let args = node
                        .child_by_field_name("arguments")
                        .map(named_children)
                        .unwrap_or_default();
                    if matches!(name.as_str(), "emptyList" | "emptyMap" | "emptySet") {
                        return Ok(Some(BTreeSet::new()));
                    }
                    if matches!(
                        name.as_str(),
                        "singletonList" | "singleton" | "asList" | "of"
                    ) {
                        let mut origins = BTreeSet::new();
                        for value in args {
                            let Some(value) = self.trace_expression(
                                method,
                                expression_from_node(&parsed.source, value),
                                value.start_byte(),
                                reachable,
                                visiting,
                                depth + 1,
                            )?
                            else {
                                return Ok(None);
                            };
                            origins.extend(value);
                        }
                        return Ok(Some(origins));
                    }
                    if name == "map"
                        && let [callback] = args.as_slice()
                    {
                        return self.trace_copy_callback(
                            method,
                            *callback,
                            &parsed.source,
                            reachable,
                            visiting,
                            depth + 1,
                        );
                    }
                    if name == "collect"
                        && let [collector] = args.as_slice()
                    {
                        let collector_site = values::invocation(&parsed.source, *collector);
                        if !collector_site.receiver.as_deref().is_some_and(|receiver| {
                            self.project
                                .imported_type(&method.file_path, receiver)
                                .as_deref()
                                == Some("java.util.stream.Collectors")
                        }) {
                            return Ok(None);
                        }
                        if collector_site.name == "toMap" {
                            if let Some(arguments) = collector.child_by_field_name("arguments")
                                && let Some(merge) = arguments.named_child(2)
                            {
                                let parameters = merge
                                    .child_by_field_name("parameters")
                                    .map(|node| {
                                        text_of(&parsed.source, node)
                                            .trim_matches(['(', ')'])
                                            .split(',')
                                            .map(str::trim)
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default();
                                if merge.kind() != "lambda_expression"
                                    || parameters.len() != 2
                                    || merge.child_by_field_name("body").is_none_or(|body| {
                                        !parameters.contains(&text_of(&parsed.source, body))
                                    })
                                {
                                    return Ok(None);
                                }
                            }
                            let mapper = collector
                                .child_by_field_name("arguments")
                                .and_then(|args| args.named_child(1));
                            if let Some(mapper) = mapper {
                                return self.trace_copy_callback(
                                    method,
                                    mapper,
                                    &parsed.source,
                                    reachable,
                                    visiting,
                                    depth + 1,
                                );
                            }
                        }
                        if !matches!(collector_site.name.as_str(), "toList" | "toSet") {
                            return Ok(None);
                        }
                    }
                    if matches!(
                        name.as_str(),
                        "stream"
                            | "filter"
                            | "collect"
                            | "toList"
                            | "values"
                            | "get"
                            | "getOrDefault"
                            | "unmodifiableList"
                            | "unmodifiableMap"
                    ) {
                        let input =
                            if matches!(name.as_str(), "unmodifiableList" | "unmodifiableMap") {
                                args.first().copied()
                            } else {
                                node.child_by_field_name("object")
                            };
                        if let Some(input) = input {
                            let Some(mut origins) = self.trace_expression(
                                method,
                                expression_from_node(&parsed.source, input),
                                input.start_byte(),
                                reachable,
                                visiting,
                                depth + 1,
                            )?
                            else {
                                return Ok(None);
                            };
                            if name == "getOrDefault"
                                && let Some(default) = args.get(1)
                            {
                                let Some(default) = self.trace_expression(
                                    method,
                                    expression_from_node(&parsed.source, *default),
                                    default.start_byte(),
                                    reachable,
                                    visiting,
                                    depth + 1,
                                )?
                                else {
                                    return Ok(None);
                                };
                                origins.extend(default);
                            }
                            return Ok(Some(origins));
                        }
                    }
                }
                self.trace_call(method, &name, offset, reachable, visiting, depth + 1, false)
            }
            Expression::Branch(expressions) => {
                let mut origins = BTreeSet::new();
                for expression in expressions {
                    let Some(value) = self.trace_expression(
                        method,
                        expression,
                        offset,
                        reachable,
                        visiting,
                        depth + 1,
                    )?
                    else {
                        return Ok(None);
                    };
                    origins.extend(value);
                }
                Ok(Some(origins))
            }
            _ => Ok(None),
        }
    }

    fn copy_collection_type(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        offset: usize,
    ) -> Result<Option<TypeRef>> {
        let mut kind = self
            .receiver_type(method, receiver, offset)?
            .and_then(|kind| parse_java_type(&kind));
        let mut file = method.file_path.clone();
        if kind.is_none()
            && let Some((base, accessor)) = receiver.rsplit_once('.')
        {
            let field = getter_signal(accessor.trim_end_matches("()"))
                .or_else(|| (!accessor.contains('(')).then(|| accessor.to_owned()));
            if let Some(field) = field
                && let Some(class) = self.expression_class(method, base, offset)?
                && let Some(node) = self
                    .project
                    .graph()
                    .contained(&class.id, "field")
                    .into_iter()
                    .find(|node| node.name == field)
            {
                kind = declared_variable_type(&node.signature, &node.name)
                    .and_then(|kind| parse_java_type(&kind));
                file = node.file_path.clone();
            }
        }
        let Some(kind) = kind else {
            return Ok(None);
        };
        Ok(self
            .project
            .imported_type(&file, &kind.name)
            .filter(|name| {
                matches!(
                    name.as_str(),
                    "java.util.List"
                        | "java.util.Set"
                        | "java.util.Collection"
                        | "java.util.Map"
                        | "java.util.ArrayList"
                        | "java.util.HashMap"
                        | "java.util.LinkedHashMap"
                )
            })
            .filter(|_| {
                self.project
                    .resolve_type(&file, &method.qualified_name, &kind)
                    .is_none()
            })
            .map(|_| kind))
    }

    fn copy_collection_escape_is_setter(
        &mut self,
        method: &GraphNode,
        site: &InvocationSite,
        collection: &str,
    ) -> Result<bool> {
        if matches!(site.name.as_str(), "unmodifiableList" | "unmodifiableMap")
            && site.receiver.as_deref().is_some_and(|receiver| {
                self.project
                    .imported_type(&method.file_path, receiver)
                    .as_deref()
                    == Some("java.util.Collections")
            })
        {
            return Ok(true);
        }
        let Some(field) = site
            .name
            .strip_prefix("set")
            .filter(|field| !field.is_empty())
            .map(|field| {
                let mut chars = field.chars();
                format!(
                    "{}{}",
                    chars.next().unwrap().to_ascii_lowercase(),
                    chars.as_str()
                )
            })
        else {
            return Ok(false);
        };
        let targets = self.resolve_invocation(method, site)?;
        let Some(target) = targets
            .first()
            .filter(|_| targets.len() == 1)
            .and_then(|id| self.project.graph().nodes.get(id))
            .cloned()
        else {
            return Ok(false);
        };
        let parsed = self.parsed(&target.file_path)?.clone();
        if let Some(declaration) = lookup::method_declaration(&parsed, &target) {
            let Some(index) = site.arguments.iter().position(
                |argument| matches!(argument, Expression::Identifier(value) if value == collection),
            ) else {
                return Ok(false);
            };
            let parameters = method_parameters(&target.signature);
            let Some((_, parameter)) = parameters.get(index) else {
                return Ok(false);
            };
            let Some(body) = declaration.child_by_field_name("body") else {
                return Ok(false);
            };
            let statements = lookup::statements(body);
            let [statement] = statements.as_slice() else {
                return Ok(false);
            };
            let Some(assignment) = statement.named_child(0) else {
                return Ok(false);
            };
            return Ok(assignment.kind() == "assignment_expression"
                && assignment.child_by_field_name("left").is_some_and(|node| {
                    text_of(&parsed.source, node) == field
                        || text_of(&parsed.source, node) == format!("this.{field}")
                })
                && assignment
                    .child_by_field_name("operator")
                    .is_some_and(|node| text_of(&parsed.source, node) == "=")
                && assignment
                    .child_by_field_name("right")
                    .is_some_and(|node| text_of(&parsed.source, node) == parameter));
        }
        let Some(receiver) = site.receiver.as_deref() else {
            return Ok(false);
        };
        let Some(class) = self.expression_class(method, receiver, site.offset)? else {
            return Ok(false);
        };
        Ok(
            values::type_declaration(&parsed, &class).is_some_and(|declaration| {
                (primary::has_annotation(&parsed.source, declaration, "Data")
                    || primary::has_annotation(&parsed.source, declaration, "Setter"))
                    && primary::instance_fields(&parsed.source, declaration).contains(&field)
            }),
        )
    }

    fn copy_pipeline(
        &mut self,
        method: &GraphNode,
        mut node: Node<'_>,
        source: &str,
    ) -> Result<bool> {
        if self.standard_pipeline(method, node, source)? {
            return Ok(true);
        }
        while node.kind() == "method_invocation" {
            let site = values::invocation(source, node);
            if !matches!(
                site.name.as_str(),
                "stream"
                    | "filter"
                    | "map"
                    | "collect"
                    | "toList"
                    | "values"
                    | "get"
                    | "getOrDefault"
            ) {
                return Ok(false);
            }
            let Some(object) = node.child_by_field_name("object") else {
                return Ok(false);
            };
            if self
                .copy_collection_type(method, text_of(source, object), object.start_byte())?
                .is_some()
            {
                return Ok(true);
            }
            node = object;
        }
        Ok(false)
    }

    fn trace_copy_callback(
        &mut self,
        method: &GraphNode,
        callback: Node<'_>,
        source: &str,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        if callback.kind() == "method_reference" {
            let parts = named_children(callback);
            let [receiver, name] = parts.as_slice() else {
                return Ok(None);
            };
            let site = InvocationSite {
                name: text_of(source, *name).to_owned(),
                receiver: Some(text_of(source, *receiver).to_owned()),
                arity: 1,
                exact_arity: true,
                arguments: Vec::new(),
                offset: callback.start_byte(),
                line: callback.start_position().row + 1,
                column: callback.start_position().column + 1,
            };
            let targets = self.resolve_invocation(method, &site)?;
            if let [target] = targets.as_slice()
                && let Some(target) = self.project.graph().nodes.get(target).cloned()
            {
                return self.trace_method_return(&target, reachable, visiting, depth + 1, false);
            }
        }
        if callback.kind() == "lambda_expression"
            && let Some(body) = callback.child_by_field_name("body")
        {
            let values = if body.kind() == "block" {
                descendants(body)
                    .into_iter()
                    .filter(|node| {
                        node.kind() == "return_statement"
                            && lookup::ancestors(*node)
                                .find(|node| node.kind() == "lambda_expression")
                                == Some(callback)
                    })
                    .filter_map(|node| node.named_child(0))
                    .collect::<Vec<_>>()
            } else {
                vec![body]
            };
            if values.is_empty() {
                return Ok(None);
            }
            let mut origins = BTreeSet::new();
            for value in values {
                let Some(value) = self.trace_expression(
                    method,
                    expression_from_node(source, value),
                    value.start_byte(),
                    reachable,
                    visiting,
                    depth + 1,
                )?
                else {
                    return Ok(None);
                };
                origins.extend(value);
            }
            return Ok(Some(origins));
        }
        Ok(None)
    }

    fn trace_parameter(
        &mut self,
        method: &GraphNode,
        index: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        let key = (format!("{}:{index}", method.id), false);
        if !visiting.insert(key.clone()) {
            return Ok(None);
        }
        let mut result = None;
        for caller_id in &reachable.nodes {
            if caller_id == &method.id {
                continue;
            }
            let Some(caller) = self.project.graph().nodes.get(caller_id).cloned() else {
                continue;
            };
            for invocation in self.method_invocations(&caller)? {
                if !invocation.exact_arity
                    && index == 0
                    && self.invocation_reaches(&caller, &invocation, &method.id)?
                {
                    let parsed = self.parsed(&caller.file_path)?.clone();
                    let reference = descendants(parsed.tree.root_node())
                        .into_iter()
                        .find(|node| {
                            node.kind() == "method_reference"
                                && node.start_byte() == invocation.offset
                        });
                    if let Some(reference) = reference
                        && let Some(map) =
                            reference
                                .parent()
                                .and_then(|node| node.parent())
                                .filter(|node| {
                                    node.kind() == "method_invocation"
                                        && values::invocation(&parsed.source, *node).name == "map"
                                })
                        && self.copy_pipeline(&caller, map, &parsed.source)?
                        && let Some(input) = map.child_by_field_name("object")
                    {
                        let Some(origins) = self.trace_expression(
                            &caller,
                            expression_from_node(&parsed.source, input),
                            input.start_byte(),
                            reachable,
                            visiting,
                            depth + 1,
                        )?
                        else {
                            visiting.remove(&key);
                            return Ok(None);
                        };
                        if result.as_ref().is_some_and(|previous| previous != &origins) {
                            visiting.remove(&key);
                            return Ok(None);
                        }
                        result = Some(origins);
                    }
                    continue;
                }
                if !invocation.exact_arity || invocation.arguments.len() <= index {
                    continue;
                }
                if !self.invocation_reaches(&caller, &invocation, &method.id)? {
                    continue;
                }
                let Some((argument, argument_offset)) =
                    self.invocation_argument(&caller, &invocation, index)?
                else {
                    visiting.remove(&key);
                    return Ok(None);
                };
                let Some(origins) = self.trace_expression(
                    &caller,
                    argument,
                    argument_offset,
                    reachable,
                    visiting,
                    depth + 1,
                )?
                else {
                    visiting.remove(&key);
                    return Ok(None);
                };
                // Without a call-site binding, different arguments must not be
                // combined into one object identity.
                if result.as_ref().is_some_and(|previous| previous != &origins) {
                    visiting.remove(&key);
                    return Ok(None);
                }
                result = Some(origins);
            }
        }
        visiting.remove(&key);
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_holder_getter(
        &mut self,
        method: &GraphNode,
        holder_receiver: &str,
        accessor: &str,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        let Some(field_name) = getter_signal(accessor) else {
            return Ok(None);
        };
        let holder_receiver = normalize_receiver(holder_receiver);
        let Some(holder_origins) = self.trace_receiver(
            method,
            &holder_receiver,
            offset,
            reachable,
            visiting,
            depth + 1,
        )?
        else {
            return Ok(None);
        };
        let Some(holder) = self.expression_class(method, &holder_receiver, offset)? else {
            return Ok(None);
        };
        let setter_name = format!("set{}", uppercase_first(&field_name));
        let mut origins = BTreeSet::new();
        let mut matched = false;
        let methods = reachable.nodes.iter().cloned().collect();
        for edge in self.typed_setter_edges(&holder, &field_name, &methods)? {
            let Some(source) = self.project.graph().nodes.get(&edge.source).cloned() else {
                continue;
            };
            let Some((setter_receiver, setter_offset)) =
                self.setter_receiver(&edge, &source, &setter_name)?
            else {
                continue;
            };
            let assigned = self.trace_receiver(
                &source,
                &setter_receiver,
                setter_offset,
                reachable,
                visiting,
                depth + 1,
            )?;
            if !assigned.is_some_and(|assigned| !assigned.is_disjoint(&holder_origins)) {
                continue;
            }
            matched = true;
            let Some((expression, _, value_offset)) = self.setter_argument(&edge, &setter_name)?
            else {
                return Ok(None);
            };
            let Some(value_origins) = self.trace_expression(
                &source,
                expression,
                value_offset,
                reachable,
                visiting,
                depth + 1,
            )?
            else {
                return Ok(None);
            };
            origins.extend(value_origins);
        }
        Ok(matched.then_some(origins))
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_call(
        &mut self,
        caller: &GraphNode,
        name: &str,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
        unwrap_wrapper: bool,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        let Some(invocation) = self
            .method_invocations(caller)?
            .into_iter()
            .find(|invocation| invocation.name == name && invocation.offset == offset)
        else {
            return Ok(None);
        };
        let mut targets = self.resolve_invocation(caller, &invocation)?;
        let graph = self.project.graph();
        let mut methods = Vec::new();
        for target in targets.drain(..) {
            let Some(method) = graph.nodes.get(&target) else {
                continue;
            };
            if let Some(implementation) = self.implementation_method(method) {
                if let Some(method) = graph.nodes.get(&implementation) {
                    methods.push(method.clone());
                    continue;
                }
            }
            methods.push(method.clone());
        }
        methods.sort_by(|left, right| left.id.cmp(&right.id));
        methods.dedup_by(|left, right| left.id == right.id);
        if methods.len() != 1 {
            return Ok(None);
        }
        self.trace_method_return(&methods[0], reachable, visiting, depth + 1, unwrap_wrapper)
    }

    fn trace_method_return(
        &mut self,
        method: &GraphNode,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
        unwrap_wrapper: bool,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        let key = (method.id.clone(), unwrap_wrapper);
        if !visiting.insert(key.clone()) {
            return Ok(None);
        }
        let returns = self.return_values(method)?;
        if returns.is_empty() {
            visiting.remove(&key);
            return Ok(None);
        }
        let mut result = BTreeSet::new();
        for value in returns {
            let candidate = if unwrap_wrapper
                && matches!(&value.expression, Expression::Call { name, source } if name == "execute" && source.contains("BaseRemoteServiceTemplate"))
            {
                Some(self.trace_template_process(method, reachable, visiting, depth + 1))
            } else if unwrap_wrapper {
                if let Some(arguments) = value.wrapper_arguments {
                    arguments.first().map(|(expression, offset)| {
                        self.trace_expression(
                            method,
                            expression.clone(),
                            *offset,
                            reachable,
                            visiting,
                            depth + 1,
                        )
                    })
                } else {
                    Some(match value.expression {
                        Expression::Identifier(receiver) => self.trace_wrapper_receiver(
                            method,
                            &receiver,
                            value.offset,
                            reachable,
                            visiting,
                            depth + 1,
                        ),
                        Expression::Call { name, .. } => self.trace_call(
                            method,
                            &name,
                            value.offset,
                            reachable,
                            visiting,
                            depth + 1,
                            true,
                        ),
                        expression => self.trace_expression(
                            method,
                            expression,
                            value.offset,
                            reachable,
                            visiting,
                            depth + 1,
                        ),
                    })
                }
            } else {
                Some(self.trace_expression(
                    method,
                    value.expression,
                    value.offset,
                    reachable,
                    visiting,
                    depth + 1,
                ))
            };
            let Some(candidate) = candidate.transpose()? else {
                visiting.remove(&key);
                return Ok(None);
            };
            let Some(candidate) = candidate else {
                visiting.remove(&key);
                return Ok(None);
            };
            result.extend(candidate);
        }
        visiting.remove(&key);
        Ok(Some(result))
    }

    fn trace_template_process(
        &mut self,
        template_method: &GraphNode,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        let parsed = self.parsed(&template_method.file_path)?;
        let Some(declaration) = lookup::method_declaration(parsed, template_method) else {
            return Ok(None);
        };
        let process_lines = descendants(declaration)
            .into_iter()
            .filter(|node| {
                node.kind() == "method_declaration"
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|name| text_of(&parsed.source, name) == "process")
            })
            .map(|node| node.start_position().row + 1)
            .collect::<BTreeSet<_>>();
        let mut candidates = self
            .project
            .graph()
            .nodes
            .values()
            .filter(|method| {
                method.kind == "method"
                    && method.name == "process"
                    && method.file_path == template_method.file_path
                    && process_lines.contains(&method.start_line)
            })
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by_key(|method| method.start_line);
        let [process] = candidates.as_slice() else {
            return Ok(None);
        };
        self.trace_method_return(process, reachable, visiting, depth + 1, false)
    }

    fn trace_wrapper_receiver(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, bool)>,
        depth: usize,
    ) -> Result<Option<BTreeSet<SetterOrigin>>> {
        let receiver = normalize_receiver(receiver);
        if let Some((value, declaration_offset)) =
            self.copy_local_value(method, &receiver, offset)?
        {
            if let Expression::Call { name, .. } = value {
                return self.trace_call(
                    method,
                    &name,
                    declaration_offset,
                    reachable,
                    visiting,
                    depth + 1,
                    true,
                );
            }
            if let Expression::Getter { receiver, accessor } = value {
                if accessor == "getData" {
                    return self.trace_wrapper_receiver(
                        method,
                        &receiver,
                        declaration_offset,
                        reachable,
                        visiting,
                        depth + 1,
                    );
                }
            }
            return Ok(None);
        }
        if receiver.ends_with("()") {
            let name = receiver
                .trim_end_matches("()")
                .rsplit('.')
                .next()
                .unwrap_or_default();
            return self.trace_call(method, name, offset, reachable, visiting, depth + 1, true);
        }
        Ok(None)
    }

    fn is_known_result_receiver(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        offset: usize,
    ) -> Result<bool> {
        if receiver.ends_with(')') {
            let parsed = self.parsed(&method.file_path)?.clone();
            if let Some(node) = values::expression_node(&parsed, method, receiver, offset) {
                let targets =
                    self.resolve_invocation(method, &values::invocation(&parsed.source, node))?;
                return Ok(targets
                    .first()
                    .filter(|_| targets.len() == 1)
                    .and_then(|id| self.project.graph().nodes.get(id))
                    .and_then(|method| parse_java_type(&method.return_type))
                    .is_some_and(|kind| KNOWN_RESULT_WRAPPERS.contains(&kind.simple_name())));
            }
        }
        let base = receiver
            .trim()
            .rsplit('.')
            .next()
            .unwrap_or(receiver)
            .trim_end_matches("()");
        let Some(type_name) = self.receiver_type(method, base, offset)? else {
            return Ok(false);
        };
        let Some(type_ref) = parse_java_type(&type_name) else {
            return Ok(false);
        };
        Ok(KNOWN_RESULT_WRAPPERS.contains(&type_ref.simple_name()))
    }

    fn invocation_argument(
        &mut self,
        method: &GraphNode,
        invocation: &InvocationSite,
        index: usize,
    ) -> Result<Option<(Expression, usize)>> {
        let parsed = self.parsed(&method.file_path)?;
        let Some(node) = descendants(parsed.tree.root_node())
            .into_iter()
            .find(|node| {
                node.kind() == "method_invocation"
                    && node.start_byte() == invocation.offset
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|name| text_of(&parsed.source, name) == invocation.name)
            })
        else {
            return Ok(None);
        };
        Ok(node
            .child_by_field_name("arguments")
            .and_then(|arguments| named_children(arguments).get(index).copied())
            .map(|argument| {
                (
                    expression_from_node(&parsed.source, argument),
                    argument.start_byte(),
                )
            }))
    }

    fn return_values(&mut self, method: &GraphNode) -> Result<Vec<ReturnValue>> {
        let parsed = self.parsed(&method.file_path)?;
        let Some(declaration) = lookup::method_declaration(parsed, method) else {
            return Ok(Vec::new());
        };
        let source = parsed.source.clone();
        let mut result = Vec::new();
        for statement in descendants(declaration)
            .into_iter()
            .filter(|node| node.kind() == "return_statement")
        {
            let Some(expression) = statement
                .child_by_field_name("expression")
                .or_else(|| named_children(statement).first().copied())
            else {
                continue;
            };
            if lookup::ancestors(statement).find(|node| {
                matches!(
                    node.kind(),
                    "method_declaration" | "constructor_declaration" | "lambda_expression"
                )
            }) != Some(declaration)
            {
                continue;
            }
            let wrapper_arguments = if expression.kind() == "method_invocation" {
                let object = expression
                    .child_by_field_name("object")
                    .map(|node| text_of(&source, node).trim().to_owned());
                let name = expression
                    .child_by_field_name("name")
                    .map(|node| text_of(&source, node).to_owned());
                if object
                    .as_deref()
                    .is_some_and(|value| KNOWN_RESULT_WRAPPERS.contains(&value))
                    && name
                        .as_deref()
                        .is_some_and(|value| KNOWN_RESULT_FACTORIES.contains(&value))
                {
                    Some(
                        expression
                            .child_by_field_name("arguments")
                            .map(named_children)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|node| (expression_from_node(&source, node), node.start_byte()))
                            .collect(),
                    )
                } else {
                    None
                }
            } else {
                None
            };
            result.push(ReturnValue {
                expression: expression_from_node(&source, expression),
                offset: expression.start_byte(),
                wrapper_arguments,
            });
        }
        Ok(result)
    }

    fn is_local_variable(&mut self, method: &GraphNode, name: &str, offset: usize) -> Result<bool> {
        if method_parameters(&method.signature)
            .iter()
            .any(|(_, parameter)| parameter == name)
        {
            return Ok(false);
        }
        let parsed = self.parsed(&method.file_path)?;
        let Some(declaration) = lookup::method_declaration(parsed, method) else {
            return Ok(false);
        };
        Ok(descendants(declaration).into_iter().any(|node| {
            node.kind() == "variable_declarator"
                && node.start_byte() < offset
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name_node| text_of(&parsed.source, name_node) == name)
        }))
    }

    fn setter_receiver(
        &mut self,
        edge: &GraphEdge,
        method: &GraphNode,
        setter_name: &str,
    ) -> Result<Option<(String, usize)>> {
        let mut candidates = self
            .method_invocations(method)?
            .into_iter()
            .filter(|invocation| invocation.name == setter_name && invocation.line == edge.line)
            .collect::<Vec<_>>();
        candidates.sort_by_key(|invocation| invocation.column.abs_diff(edge.column));
        Ok(candidates.into_iter().next().and_then(|invocation| {
            invocation
                .receiver
                .map(|receiver| (receiver, invocation.offset))
        }))
    }
}

fn normalize_receiver(receiver: &str) -> String {
    // A field and a shadowing local are different objects.
    receiver.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphEdge, GraphNode, Reachability, test_snapshot};
    use crate::model::{HttpRoute, RouteSource, RouteStatus, TypeRef};
    use std::collections::{HashMap, HashSet};
    use std::fs;

    fn node(
        id: &str,
        kind: &str,
        name: &str,
        qualified_name: &str,
        file_path: &str,
        start_line: usize,
        signature: &str,
    ) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            file_path: file_path.to_owned(),
            start_line,
            start_column: 0,
            docstring: None,
            signature: signature.to_owned(),
            decorators: String::new(),
            return_type: String::new(),
        }
    }

    fn edge(source: &str, target: &str, kind: &str, line: usize, column: usize) -> GraphEdge {
        GraphEdge {
            source: source.to_owned(),
            target: target.to_owned(),
            kind: kind.to_owned(),
            line,
            column,
            metadata: String::new(),
            provenance: String::new(),
        }
    }

    fn operation() -> Operation {
        Operation {
            key: "Facade#query".to_owned(),
            facade_name: "Facade".to_owned(),
            facade_fqn: "p.Facade".to_owned(),
            method_name: "query".to_owned(),
            signature: "DTO ()".to_owned(),
            description: None,
            contract_source: "Writer.java".to_owned(),
            request: None,
            request_arguments: Vec::new(),
            response: TypeRef {
                name: "p.DTO".to_owned(),
                arguments: Vec::new(),
                array_depth: 0,
            },
            request_schema: None,
            response_schema: None,
            service: None,
            route: HttpRoute {
                status: RouteStatus::Placeholder,
                source: RouteSource::Placeholder,
                method: "POST".to_owned(),
                path: "/query".to_owned(),
                host: None,
            },
            semantic_patches: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn copy_local_requires_definite_single_assignment() {
        for (assignment, expected) in [
            ("value = source.query();", true),
            (
                "try { value = source.query(); } catch (Exception error) { throw error; }",
                true,
            ),
            (
                "try { value = source.query(); } catch (Exception error) { log(error); }",
                false,
            ),
            ("if (ready) { value = source.query(); }", false),
            ("value = source.query(); value = source.other();", false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let source = format!(
                "package p; class Writer {{ void run() {{ DTO value; {assignment} use(value); }} }}"
            );
            fs::write(root.path().join("Writer.java"), &source).unwrap();
            let method = node(
                "run",
                "method",
                "run",
                "p::Writer::run",
                "Writer.java",
                1,
                "void ()",
            );
            let graph = test_snapshot(
                vec![
                    node(
                        "writer",
                        "class",
                        "Writer",
                        "p::Writer",
                        "Writer.java",
                        1,
                        "",
                    ),
                    method.clone(),
                ],
                vec![edge("writer", "run", "contains", 1, 0)],
            );
            let project = JavaProject::load(root.path(), &graph).unwrap();
            let mut analyzer = SemanticAnalyzer::new(&project);
            let value = analyzer
                .copy_local_value(&method, "value", source.find("use(value)").unwrap())
                .unwrap();
            assert_eq!(
                matches!(value, Some((Expression::Call { name, .. }, _)) if name == "query"),
                expected,
                "{assignment}"
            );
        }
    }

    #[test]
    fn follows_returned_dto_and_excludes_scratch_instance() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("DTO.java"),
            "package p; class DTO { void setCode(String code) {} String getCode() { return \"\"; } }",
        )
        .unwrap();
        fs::write(
            root.path().join("Writer.java"),
            "package p; class Writer { Source source; void toResp(DTO workOrder) { workOrder.getCode(); } void run() { DTO result = source.query(); toResp(result); } }",
        )
        .unwrap();
        fs::write(
            root.path().join("Source.java"),
            "package p; class Source { DTO query() { DTO scratch = new DTO(); scratch.setCode(\"scratch\"); DTO returned = new DTO(); returned.setCode(\"returned\"); return returned; } }",
        )
        .unwrap();

        let graph = test_snapshot(
            vec![
                node("dto", "class", "DTO", "p::DTO", "DTO.java", 1, ""),
                node(
                    "setter",
                    "method",
                    "setCode",
                    "p::DTO::setCode",
                    "DTO.java",
                    1,
                    "void (String code)",
                ),
                node(
                    "writer",
                    "class",
                    "Writer",
                    "p::Writer",
                    "Writer.java",
                    1,
                    "",
                ),
                node(
                    "source-field",
                    "field",
                    "source",
                    "p::Writer::source",
                    "Writer.java",
                    1,
                    "Source source",
                ),
                node(
                    "to-resp",
                    "method",
                    "toResp",
                    "p::Writer::toResp",
                    "Writer.java",
                    1,
                    "void (DTO workOrder)",
                ),
                node(
                    "run",
                    "method",
                    "run",
                    "p::Writer::run",
                    "Writer.java",
                    1,
                    "void ()",
                ),
                node(
                    "source",
                    "class",
                    "Source",
                    "p::Source",
                    "Source.java",
                    1,
                    "",
                ),
                node(
                    "query",
                    "method",
                    "query",
                    "p::Source::query",
                    "Source.java",
                    1,
                    "DTO ()",
                ),
            ],
            vec![
                edge("dto", "setter", "contains", 1, 0),
                edge("writer", "source-field", "contains", 1, 0),
                edge("writer", "to-resp", "contains", 1, 0),
                edge("writer", "run", "contains", 1, 0),
                edge("source", "query", "contains", 1, 0),
                edge("run", "to-resp", "calls", 1, 72),
                edge("run", "query", "calls", 1, 43),
                edge("query", "setter", "calls", 1, 69),
            ],
        );
        let project = JavaProject::load(root.path(), &graph).unwrap();
        let writer = graph.nodes.get("to-resp").unwrap().clone();
        let reachable = Reachability {
            nodes: HashSet::from([
                "to-resp".to_owned(),
                "run".to_owned(),
                "query".to_owned(),
                "setter".to_owned(),
            ]),
            parent: HashMap::new(),
        };
        let mut analyzer = SemanticAnalyzer::new(&project);
        let origins = analyzer
            .copied_field_origins(&operation(), &writer, "workOrder", 70, &reachable)
            .unwrap();
        assert_eq!(origins.len(), 1);
        assert!(
            origins
                .iter()
                .any(|(method, receiver)| method == "query" && receiver.starts_with("returned@"))
        );
        assert!(
            analyzer
                .copied_field_origins(&operation(), &writer, "this.workOrder", 70, &reachable)
                .unwrap()
                .is_empty()
        );
        fs::write(
            root.path().join("Writer.java"),
            "package p; class Writer { Source source; void toResp(DTO workOrder) { workOrder.getCode(); } void run() { DTO result = source.query(); toResp(result); DTO scratch = new DTO(); toResp(scratch); } }",
        )
        .unwrap();
        let project = JavaProject::load(root.path(), &graph).unwrap();
        let mut analyzer = SemanticAnalyzer::new(&project);
        assert!(
            analyzer
                .copied_field_origins(&operation(), &writer, "workOrder", 70, &reachable)
                .unwrap()
                .is_empty(),
            "different call arguments cannot share an object identity"
        );
    }

    #[test]
    fn follows_holder_getter_only_for_same_holder_instance() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("DTO.java"), "package p; class DTO {}").unwrap();
        fs::write(
            root.path().join("Holder.java"),
            "package p; class Holder { void setWorkOrder(DTO value) {} DTO getWorkOrder() { return null; } }",
        )
        .unwrap();
        fs::write(
            root.path().join("Writer.java"),
            "package p;
class Writer {
 DTO toResp(Holder context) {
  return context.getWorkOrder();
 }
 void run() {
  Holder good = new Holder();
  DTO dto = new DTO();
  good.setWorkOrder(dto);
  Holder bad = new Holder();
  DTO other = new DTO();
  bad.setWorkOrder(other);
  toResp(good);
 }
}",
        )
        .unwrap();

        let graph = test_snapshot(
            vec![
                node("dto", "class", "DTO", "p::DTO", "DTO.java", 1, ""),
                node(
                    "holder",
                    "class",
                    "Holder",
                    "p::Holder",
                    "Holder.java",
                    1,
                    "",
                ),
                node(
                    "set-work-order",
                    "method",
                    "setWorkOrder",
                    "p::Holder::setWorkOrder",
                    "Holder.java",
                    1,
                    "void (DTO value)",
                ),
                node(
                    "writer",
                    "class",
                    "Writer",
                    "p::Writer",
                    "Writer.java",
                    2,
                    "",
                ),
                node(
                    "to-resp",
                    "method",
                    "toResp",
                    "p::Writer::toResp",
                    "Writer.java",
                    3,
                    "DTO (Holder context)",
                ),
                node(
                    "run",
                    "method",
                    "run",
                    "p::Writer::run",
                    "Writer.java",
                    6,
                    "void ()",
                ),
            ],
            vec![
                edge("holder", "set-work-order", "contains", 1, 0),
                edge("writer", "to-resp", "contains", 3, 1),
                edge("writer", "run", "contains", 6, 1),
                edge("run", "set-work-order", "calls", 9, 2),
                edge("run", "set-work-order", "calls", 12, 2),
                edge("run", "to-resp", "calls", 13, 2),
            ],
        );
        let project = JavaProject::load(root.path(), &graph).unwrap();
        let writer = graph.nodes.get("to-resp").unwrap().clone();
        let reachable = Reachability {
            nodes: HashSet::from([
                "to-resp".to_owned(),
                "run".to_owned(),
                "set-work-order".to_owned(),
            ]),
            parent: HashMap::new(),
        };
        let mut analyzer = SemanticAnalyzer::new(&project);
        let origins = analyzer
            .copied_field_origins(
                &operation(),
                &writer,
                "context.getWorkOrder()",
                61,
                &reachable,
            )
            .unwrap();
        assert_eq!(origins.len(), 1);
        assert!(
            origins
                .iter()
                .any(|(method, receiver)| method == "run" && receiver.starts_with("dto@"))
        );
    }
}
