use super::*;

impl SemanticAnalyzer<'_> {
    pub(super) fn enum_projection(
        &mut self,
        method: &GraphNode,
        expression: &Expression,
        offset: usize,
    ) -> Result<Option<Domain>> {
        let projection = match expression {
            Expression::Getter { receiver, accessor } => {
                Some((receiver.as_str(), accessor.as_str(), false))
            }
            Expression::Call { name, source } => source
                .strip_suffix(&format!(".{name}()"))
                .map(|receiver| (receiver, name.as_str(), false)),
            Expression::Identifier(value) => value
                .rsplit_once('.')
                .map(|(receiver, member)| (receiver, member, true)),
            _ => None,
        };
        let Some((receiver, accessor, field)) = projection else {
            return Ok(None);
        };
        let Some(class) = self.enum_for_receiver(method, receiver, offset)? else {
            return Ok(None);
        };
        if (field && self.enum_member(&class, accessor)?) || (!field && accessor == "values") {
            return self.serialized_enum_domain(&class).map(Some);
        }
        if field
            && let Expression::Identifier(value) = expression
            && let Some(literal) = self.constant_literal(method, value)?
        {
            return Ok(Some(Domain {
                literals: BTreeSet::from([literal]),
                ..Domain::default()
            }));
        }
        let mut domain = self.enum_domain(&class, accessor)?;
        if let Some((_, constant)) = receiver.rsplit_once('.')
            && self.enum_member(&class, constant)?
        {
            domain.enum_constants = Some(BTreeSet::from([constant.to_owned()]));
        }
        Ok(Some(domain))
    }

    /// Resolve the declared receiver type, including enum factories and chained calls.
    pub(super) fn expression_class(
        &mut self,
        method: &GraphNode,
        text: &str,
        offset: usize,
    ) -> Result<Option<GraphNode>> {
        let text = text.trim();
        if text == "this" {
            return Ok(self.lexical_owners(method).first().copied().cloned());
        }
        if let Some(domain) = self
            .value_bindings
            .iter()
            .rev()
            .find_map(|bindings| bindings.get(&(method.id.clone(), text.to_owned())))
            && let Some(fqn) = &domain.enum_fqn
            && !method_parameters(&method.signature)
                .iter()
                .any(|(kind, name)| {
                    name == text && parse_java_type(kind).is_some_and(|kind| is_scalar(&kind))
                })
        {
            return Ok(self.project.node_for_fqn(fqn).cloned());
        }
        if !text.contains(['(', ')']) {
            let owner = method
                .qualified_name
                .rsplit_once("::")
                .map(|(owner, _)| owner)
                .unwrap_or(&method.qualified_name);
            let direct = parse_java_type(text)
                .and_then(|kind| self.project.resolve_type(&method.file_path, owner, &kind))
                .cloned();
            if direct.is_some() {
                return Ok(direct);
            }
            if let Some((base, member)) = text.rsplit_once('.')
                && let Some(owner) = self.expression_class(method, base, offset)?
            {
                if owner.kind == "enum" && self.enum_member(&owner, member)? {
                    return Ok(Some(owner));
                }
                if let Some(field) = self
                    .project
                    .graph()
                    .contained(&owner.id, "field")
                    .into_iter()
                    .find(|node| node.name == member)
                    && let Some(kind) = declared_variable_type(&field.signature, &field.name)
                        .and_then(|kind| parse_java_type(&kind))
                {
                    return Ok(self
                        .project
                        .resolve_type(&field.file_path, &owner.qualified_name, &kind)
                        .cloned());
                }
                return Ok(None);
            }
            let root = text.strip_prefix("this.").unwrap_or(text);
            let kind = self
                .receiver_type(method, root, offset)?
                .and_then(|kind| parse_java_type(&kind));
            return Ok(kind
                .and_then(|kind| self.project.resolve_type(&method.file_path, owner, &kind))
                .cloned());
        }
        let parsed = self.parsed(&method.file_path)?.clone();
        let Some(node) = expression_node(&parsed, method, text, offset) else {
            return Ok(None);
        };
        if node.kind() == "object_creation_expression" {
            return Ok(node
                .child_by_field_name("type")
                .and_then(|kind| parse_java_type(text_of(&parsed.source, kind)))
                .and_then(|kind| {
                    self.project
                        .resolve_type(&method.file_path, &method.qualified_name, &kind)
                })
                .cloned());
        }
        if node.kind() != "method_invocation" {
            return Ok(None);
        }
        let site = invocation(&parsed.source, node);
        if matches!(site.name.as_str(), "getKey" | "getValue")
            && site.arity == 0
            && let Some(receiver) = site.receiver.as_deref()
            && let Some(kind) = self
                .receiver_type(method, receiver, offset)?
                .and_then(|kind| parse_java_type(&kind))
            && self
                .project
                .resolve_type(&method.file_path, &method.qualified_name, &kind)
                .is_none()
            && (kind.name == "java.util.Map.Entry"
                || kind.name == "Map.Entry"
                    && self
                        .project
                        .imported_type(&method.file_path, "Map")
                        .as_deref()
                        == Some("java.util.Map")
                || self
                    .project
                    .imported_type(&method.file_path, &kind.name)
                    .as_deref()
                    == Some("java.util.Map.Entry"))
            && let Some(value) = kind.arguments.get(usize::from(site.name == "getValue"))
        {
            return Ok(self
                .project
                .resolve_type(&method.file_path, &method.qualified_name, value)
                .cloned());
        }
        let targets = self.resolve_invocation(method, &site)?;
        let Some(target) = targets
            .first()
            .filter(|_| targets.len() == 1)
            .and_then(|id| self.project.graph().nodes.get(id))
        else {
            return Ok(None);
        };
        Ok(parse_java_type(&target.return_type)
            .and_then(|kind| {
                self.project
                    .resolve_type(&target.file_path, &target.qualified_name, &kind)
            })
            .cloned())
    }

    pub(super) fn enum_member(&mut self, class: &GraphNode, member: &str) -> Result<bool> {
        let parsed = self.parsed(&class.file_path)?.clone();
        Ok(type_declaration(&parsed, class)
            .and_then(|node| node.child_by_field_name("body"))
            .is_some_and(|body| {
                named_children(body)
                    .into_iter()
                    .filter(|node| node.kind() == "enum_constant")
                    .any(|node| {
                        node.child_by_field_name("name")
                            .is_some_and(|name| text_of(&parsed.source, name) == member)
                    })
            }))
    }

    pub(super) fn serialized_enum_domain(&mut self, class: &GraphNode) -> Result<Domain> {
        let parsed = self.parsed(&class.file_path)?.clone();
        let Some(declaration) = type_declaration(&parsed, class) else {
            return Ok(incomplete_enum_domain(
                class,
                "",
                "enum declaration missing",
            ));
        };
        if primary::has_annotation(&parsed.source, declaration, "JsonSerialize")
            || primary::has_annotation(&parsed.source, declaration, "JsonFormat")
            || descendants(declaration).into_iter().any(|node| {
                ["JsonProperty", "JsonAlias", "JsonCreator"]
                    .iter()
                    .any(|annotation| primary::has_annotation(&parsed.source, node, annotation))
            })
        {
            return Ok(incomplete_enum_domain(
                class,
                "",
                "custom enum serialization is not statically proven",
            ));
        }
        let annotated = ["field_declaration", "method_declaration"]
            .into_iter()
            .flat_map(|kind| primary::owned_nodes(declaration, kind))
            .filter(|node| primary::has_annotation(&parsed.source, *node, "JsonValue"))
            .collect::<Vec<_>>();
        if annotated.is_empty() {
            return self.enum_domain(class, "name");
        }
        let fields = primary::instance_fields(&parsed.source, declaration);
        if let Some(field) = primary::field_name(&parsed.source, declaration, &fields) {
            return self.enum_domain(class, &field);
        }
        Ok(incomplete_enum_domain(
            class,
            "",
            "JsonValue projection is not statically proven",
        ))
    }

    pub(super) fn computed_domain(
        &mut self,
        operation: &Operation,
        method: &GraphNode,
        text: &str,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, usize)>,
    ) -> Result<Option<Domain>> {
        let parsed = self.parsed(&method.file_path)?.clone();
        let Some(node) = expression_node(&parsed, method, text, offset) else {
            return Ok(None);
        };
        if node.kind() != "method_invocation" {
            return Ok(None);
        }
        let site = invocation(&parsed.source, node);
        let args = node
            .child_by_field_name("arguments")
            .map(named_children)
            .unwrap_or_default();
        if self.standard_pipeline(method, node, &parsed.source)? {
            match site.name.as_str() {
                "of" | "ofNullable" | "asList" | "singletonList" | "singleton" => {
                    let mut result = Domain::default();
                    for arg in args {
                        merge_domain(
                            &mut result,
                            self.analyze_expression(
                                operation,
                                method,
                                expression_from_node(&parsed.source, arg),
                                arg.start_byte(),
                                reachable,
                                visiting,
                            )?,
                        );
                    }
                    return Ok(Some(result));
                }
                "map" if args.len() == 1 => {
                    let object = node
                        .child_by_field_name("object")
                        .expect("pipeline receiver");
                    let input = self.analyze_expression(
                        operation,
                        method,
                        expression_from_node(&parsed.source, object),
                        object.start_byte(),
                        reachable,
                        visiting,
                    )?;
                    return self
                        .callback_domain(
                            operation,
                            method,
                            args[0],
                            &parsed.source,
                            input,
                            reachable,
                            visiting,
                        )
                        .map(Some);
                }
                "orElse" | "orElseGet" => {
                    let object = node
                        .child_by_field_name("object")
                        .expect("pipeline receiver");
                    let mut result = self.analyze_expression(
                        operation,
                        method,
                        expression_from_node(&parsed.source, object),
                        object.start_byte(),
                        reachable,
                        visiting,
                    )?;
                    if let Some(arg) = args.first() {
                        let fallback = if site.name == "orElseGet" {
                            self.callback_domain(
                                operation,
                                method,
                                *arg,
                                &parsed.source,
                                Domain::default(),
                                reachable,
                                visiting,
                            )?
                        } else {
                            self.analyze_expression(
                                operation,
                                method,
                                expression_from_node(&parsed.source, *arg),
                                arg.start_byte(),
                                reachable,
                                visiting,
                            )?
                        };
                        merge_domain(&mut result, fallback);
                    }
                    return Ok(Some(result));
                }
                "collect" | "toList" | "stream" | "filter" | "distinct" | "findFirst" | "get"
                | "orElseThrow" => {
                    let object = node
                        .child_by_field_name("object")
                        .expect("pipeline receiver");
                    let input = if site.name == "stream" && args.len() == 1 {
                        args[0]
                    } else {
                        object
                    };
                    let mut domain = self.analyze_expression(
                        operation,
                        method,
                        expression_from_node(&parsed.source, input),
                        input.start_byte(),
                        reachable,
                        visiting,
                    )?;
                    if site.name == "collect"
                        && !args.first().is_some_and(|collector| {
                            let collector = invocation(&parsed.source, *collector);
                            collector.receiver.as_deref().is_some_and(|receiver| {
                                self.project
                                    .imported_type(&method.file_path, receiver)
                                    .as_deref()
                                    == Some("java.util.stream.Collectors")
                            }) && matches!(
                                collector.name.as_str(),
                                "toList"
                                    | "toSet"
                                    | "toUnmodifiableList"
                                    | "toUnmodifiableSet"
                                    | "toCollection"
                            )
                        })
                    {
                        domain.transformed = true;
                        domain.unknown.insert(
                            "collector does not preserve individual enum values".to_owned(),
                        );
                    }
                    return Ok(Some(domain));
                }
                _ => {}
            }
        }
        let targets = self.resolve_invocation(method, &site)?;
        let Some(target) = targets
            .first()
            .filter(|_| targets.len() == 1)
            .and_then(|id| self.project.graph().nodes.get(id))
            .cloned()
        else {
            return Ok(None);
        };
        let key = (format!("return:{}", target.id), 0);
        if visiting.len() >= 32 || !visiting.insert(key.clone()) {
            return Ok(Some(unknown(format!(
                "return cycle:{}",
                target.qualified_name
            ))));
        }
        let result = (|| {
            let mut bindings = HashMap::new();
            for ((_, parameter), argument) in method_parameters(&target.signature).iter().zip(&args)
            {
                bindings.insert(
                    (target.id.clone(), parameter.clone()),
                    self.analyze_expression(
                        operation,
                        method,
                        expression_from_node(&parsed.source, *argument),
                        argument.start_byte(),
                        reachable,
                        visiting,
                    )?,
                );
            }
            let target_source = self.parsed(&target.file_path)?.clone();
            let Some(declaration) = lookup::method_declaration(&target_source, &target) else {
                return Ok(None);
            };
            let returns = descendants(declaration)
                .into_iter()
                .filter(|node| node.kind() == "return_statement")
                .filter(|node| {
                    lookup::ancestors(*node).find(|ancestor| {
                        matches!(
                            ancestor.kind(),
                            "method_declaration" | "lambda_expression" | "constructor_declaration"
                        )
                    }) == Some(declaration)
                })
                .filter_map(|node| named_children(node).into_iter().next())
                .collect::<Vec<_>>();
            if returns.is_empty() {
                return Ok(None);
            }
            self.value_bindings.push(bindings);
            let result = (|| {
                let mut result = Domain::default();
                for value in returns {
                    merge_domain(
                        &mut result,
                        self.analyze_expression(
                            operation,
                            &target,
                            expression_from_node(&target_source.source, value),
                            value.start_byte(),
                            reachable,
                            visiting,
                        )?,
                    );
                }
                result
                    .evidence
                    .push(format!("return:{}:{}", target.file_path, target.start_line));
                Ok(Some(result))
            })();
            self.value_bindings.pop();
            result
        })();
        visiting.remove(&key);
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn callback_domain(
        &mut self,
        operation: &Operation,
        method: &GraphNode,
        callback: Node<'_>,
        source: &str,
        input: Domain,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, usize)>,
    ) -> Result<Domain> {
        if callback.kind() == "method_reference" {
            let parts = named_children(callback);
            if let [receiver, accessor] = parts.as_slice()
                && let Some(class) = self.expression_class(
                    method,
                    text_of(source, *receiver),
                    callback.start_byte(),
                )?
                && class.kind == "enum"
            {
                return self.enum_domain(&class, text_of(source, *accessor));
            }
            return Ok(unknown(format!(
                "unresolved callback:{}",
                text_of(source, callback)
            )));
        }
        if callback.kind() != "lambda_expression" {
            return Ok(unknown(text_of(source, callback)));
        }
        let Some(body) = callback.child_by_field_name("body") else {
            return Ok(unknown("missing lambda body"));
        };
        let parameter = callback
            .child_by_field_name("parameters")
            .map(|node| {
                text_of(source, node)
                    .trim_matches(['(', ')'])
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        let value = if body.kind() == "block" {
            let statements = lookup::statements(body);
            if statements.len() != 1 || statements[0].kind() != "return_statement" {
                return Ok(unknown("callback control flow is not proven"));
            }
            named_children(statements[0])
                .into_iter()
                .next()
                .unwrap_or(body)
        } else {
            body
        };
        self.value_bindings
            .push(HashMap::from([((method.id.clone(), parameter), input)]));
        let result = self.analyze_expression(
            operation,
            method,
            expression_from_node(source, value),
            value.start_byte(),
            reachable,
            visiting,
        );
        self.value_bindings.pop();
        result
    }

    /// Only apply library propagation to a proven java.util pipeline, never a same-name business API.
    pub(super) fn standard_pipeline(
        &mut self,
        method: &GraphNode,
        mut node: Node<'_>,
        source: &str,
    ) -> Result<bool> {
        while node.kind() == "method_invocation" {
            let Some(object) = node.child_by_field_name("object") else {
                return Ok(false);
            };
            if object.kind() == "method_invocation" {
                node = object;
                continue;
            }
            let name = text_of(source, object);
            let kind = self
                .receiver_type(method, name, node.start_byte())?
                .unwrap_or_else(|| name.to_owned());
            let Some(kind) = parse_java_type(&kind) else {
                return Ok(false);
            };
            let imported = self.project.imported_type(&method.file_path, &kind.name);
            return Ok(imported.is_some_and(|fqn| {
                matches!(
                    fqn.as_str(),
                    "java.util.Optional"
                        | "java.util.Arrays"
                        | "java.util.List"
                        | "java.util.Set"
                        | "java.util.Collection"
                        | "java.util.Collections"
                        | "java.util.stream.Stream"
                )
            }) && self
                .project
                .resolve_type(&method.file_path, &method.qualified_name, &kind)
                .is_none());
        }
        Ok(false)
    }
}

pub(super) fn invocation(source: &str, node: Node<'_>) -> InvocationSite {
    let arguments = node
        .child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default();
    InvocationSite {
        name: node
            .child_by_field_name("name")
            .map(|name| text_of(source, name).to_owned())
            .unwrap_or_default(),
        receiver: node
            .child_by_field_name("object")
            .map(|object| text_of(source, object).to_owned()),
        arity: arguments.len(),
        exact_arity: true,
        arguments: arguments
            .into_iter()
            .map(|arg| expression_from_node(source, arg))
            .collect(),
        offset: node.start_byte(),
        line: node.start_position().row + 1,
        column: source[..node.start_byte()]
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count(),
    }
}

pub(super) fn expression_node<'a>(
    parsed: &'a ParsedFile,
    method: &GraphNode,
    text: &str,
    offset: usize,
) -> Option<Node<'a>> {
    let declaration = lookup::method_declaration(parsed, method)?;
    descendants(declaration)
        .into_iter()
        .filter(|node| text_of(&parsed.source, *node).trim() == text)
        .min_by_key(|node| node.start_byte().abs_diff(offset))
}

pub(super) fn type_declaration<'a>(parsed: &'a ParsedFile, class: &GraphNode) -> Option<Node<'a>> {
    descendants(parsed.tree.root_node())
        .into_iter()
        .find(|node| {
            matches!(
                node.kind(),
                "class_declaration" | "interface_declaration" | "enum_declaration"
            ) && node.start_position().row + 1 == class.start_line
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name| text_of(&parsed.source, name) == class.name)
        })
}

pub(super) fn unknown(reason: impl Into<String>) -> Domain {
    Domain {
        unknown: BTreeSet::from([reason.into()]),
        ..Domain::default()
    }
}
