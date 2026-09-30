use super::values::{invocation, type_declaration, unknown};
use super::*;

impl SemanticAnalyzer<'_> {
    pub(super) fn field_write_domains(
        &mut self,
        operation: &Operation,
        class: &GraphNode,
        field: &str,
        reachable: &Reachability,
    ) -> Result<Vec<Domain>> {
        let class_source = self.parsed(&class.file_path)?.clone();
        let Some(class_declaration) = type_declaration(&class_source, class) else {
            return Ok(Vec::new());
        };
        let instance_fields = primary::instance_fields(&class_source.source, class_declaration);
        let setter = format!("set{}", uppercase_first(field));
        let rewritten_setter =
            primary::setter_transforms(&class_source.source, class_declaration, field);
        let builder = primary::has_annotation(&class_source.source, class_declaration, "Builder");
        let methods = reachable
            .nodes
            .iter()
            .filter_map(|id| self.project.graph().nodes.get(id))
            .cloned()
            .collect::<Vec<_>>();
        let mut domains = Vec::new();
        for method in methods {
            // The call site already supplies this parameter, including method-reference callbacks.
            if method.name == setter
                && self
                    .project
                    .graph()
                    .contained(&class.id, "method")
                    .iter()
                    .any(|candidate| candidate.id == method.id)
            {
                continue;
            }
            let parsed = self.parsed(&method.file_path)?.clone();
            let Some(declaration) = lookup::method_declaration(&parsed, &method) else {
                continue;
            };
            let mut proven_sites = BTreeSet::new();
            for node in descendants(declaration) {
                let mut expression = None;
                let mut receiver = None;
                match node.kind() {
                    "method_invocation" => {
                        let site = invocation(&parsed.source, node);
                        if site.receiver.as_deref().is_some_and(|receiver| {
                            unused_receiver(&parsed.source, declaration, class, receiver, &setter)
                        }) {
                            continue;
                        }
                        if let Some(domain) = self.framework_copy_domain(
                            operation, &method, class, field, &site, reachable,
                        )? {
                            domains.push(domain);
                        }
                        if site.name == setter && site.arguments.len() == 1 {
                            let text = site.receiver.as_deref().unwrap_or("this");
                            if self
                                .expression_class(&method, text, node.start_byte())?
                                .is_some_and(|owner| owner.id == class.id)
                            {
                                if rewritten_setter {
                                    domains
                                        .push(unknown(format!("setter transforms field:{setter}")));
                                    continue;
                                }
                                expression = node
                                    .child_by_field_name("arguments")
                                    .and_then(|args| args.named_child(0));
                                receiver = Some(text.to_owned());
                            } else if self
                                .expression_class(&method, text, node.start_byte())?
                                .is_none()
                                && self.project.graph().outgoing(&method.id).any(|edge| {
                                    edge.kind == "calls"
                                        && edge.line == site.line
                                        && self.project.graph().nodes.get(&edge.target).is_some_and(
                                            |target| {
                                                target.name == setter
                                                    && self
                                                        .project
                                                        .graph()
                                                        .contained(&class.id, "method")
                                                        .iter()
                                                        .any(|method| method.id == target.id)
                                            },
                                        )
                                })
                            {
                                domains.push(unknown(format!(
                                    "unproven setter receiver:{text}.{setter}"
                                )));
                            }
                        } else if site.name == "build" && site.arguments.is_empty() {
                            let mut chain = node.child_by_field_name("object");
                            let mut value = None;
                            while let Some(call) =
                                chain.filter(|call| call.kind() == "method_invocation")
                            {
                                let call_site = invocation(&parsed.source, call);
                                if call_site.name == field
                                    && call_site.arguments.len() == 1
                                    && value.is_none()
                                {
                                    value = call
                                        .child_by_field_name("arguments")
                                        .and_then(|args| args.named_child(0));
                                }
                                if call_site.name == "builder"
                                    && call_site.arguments.is_empty()
                                    && let Some(owner) = call_site.receiver.as_deref()
                                    && self
                                        .expression_class(&method, owner, call.start_byte())?
                                        .is_some_and(|owner| owner.id == class.id)
                                {
                                    expression = if builder {
                                        value
                                    } else {
                                        self.explicit_builder_value(
                                            &method,
                                            class,
                                            field,
                                            node,
                                            &parsed.source,
                                        )?
                                    };
                                    break;
                                }
                                chain = call.child_by_field_name("object");
                            }
                        }
                    }
                    "object_creation_expression" => {
                        let Some(kind) = node
                            .child_by_field_name("type")
                            .and_then(|kind| parse_java_type(text_of(&parsed.source, kind)))
                        else {
                            continue;
                        };
                        if self
                            .project
                            .resolve_type(&method.file_path, &method.qualified_name, &kind)
                            .is_none_or(|owner| owner.id != class.id)
                        {
                            continue;
                        }
                        if node
                            .parent()
                            .filter(|node| node.kind() == "variable_declarator")
                            .and_then(|node| node.child_by_field_name("name"))
                            .is_some_and(|name| {
                                unused_receiver(
                                    &parsed.source,
                                    declaration,
                                    class,
                                    text_of(&parsed.source, name),
                                    &setter,
                                )
                            })
                        {
                            continue;
                        }
                        let args = node
                            .child_by_field_name("arguments")
                            .map(named_children)
                            .unwrap_or_default();
                        if args.is_empty()
                            && !has_definite_write(
                                &parsed.source,
                                declaration,
                                node,
                                field,
                                &setter,
                            )
                        {
                            let field_node = self
                                .project
                                .graph()
                                .contained(&class.id, "field")
                                .into_iter()
                                .find(|node| node.name == field);
                            let primitive = field_node
                                .and_then(|node| {
                                    declared_variable_type(&node.signature, &node.name)
                                })
                                .is_some_and(|kind| {
                                    matches!(
                                        kind.as_str(),
                                        "int"
                                            | "short"
                                            | "long"
                                            | "byte"
                                            | "float"
                                            | "double"
                                            | "char"
                                    )
                                });
                            domains.push(Domain {
                                literals: BTreeSet::from([
                                    if primitive { "0" } else { "null" }.into()
                                ]),
                                ..Domain::default()
                            });
                            for constructor in self
                                .project
                                .graph()
                                .contained(&class.id, "method")
                                .into_iter()
                                .filter(|method| {
                                    method.name == class.name
                                        && method_parameters(&method.signature).is_empty()
                                })
                            {
                                if let Some(declaration) =
                                    lookup::method_declaration(&class_source, constructor)
                                {
                                    for assignment in
                                        descendants(declaration).into_iter().filter(|node| {
                                            node.kind() == "assignment_expression"
                                                && node.child_by_field_name("left").is_some_and(
                                                    |left| {
                                                        text_of(&class_source.source, left)
                                                            == format!("this.{field}")
                                                    },
                                                )
                                        })
                                    {
                                        if let Some(value) = assignment.child_by_field_name("right")
                                        {
                                            domains.push(self.analyze_expression(
                                                operation,
                                                constructor,
                                                expression_from_node(&class_source.source, value),
                                                value.start_byte(),
                                                reachable,
                                                &mut BTreeSet::new(),
                                            )?);
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(index) = primary::argument_index(
                            &class_source.source,
                            class_declaration,
                            field,
                            &instance_fields,
                            args.len(),
                        ) {
                            expression = args.get(index).copied();
                        } else if !args.is_empty()
                            && primary::owned_nodes(class_declaration, "constructor_declaration")
                                .iter()
                                .any(|constructor| {
                                    text_of(&class_source.source, *constructor)
                                        .contains(&format!("this.{field}"))
                                })
                        {
                            domains.push(unknown(format!(
                                "constructor field binding is not proven:{}.{}",
                                class.name, field
                            )));
                        }
                    }
                    "assignment_expression" => {
                        let Some(left) = node.child_by_field_name("left") else {
                            continue;
                        };
                        let Some((object, member)) = text_of(&parsed.source, left).rsplit_once('.')
                        else {
                            continue;
                        };
                        if member == field
                            && self
                                .expression_class(&method, object, node.start_byte())?
                                .is_some_and(|owner| owner.id == class.id)
                        {
                            receiver = Some(object.to_owned());
                            expression = node.child_by_field_name("right");
                            if node
                                .child_by_field_name("operator")
                                .is_none_or(|operator| text_of(&parsed.source, operator) != "=")
                            {
                                domains.push(unknown(format!(
                                    "compound field write:{}",
                                    text_of(&parsed.source, node)
                                )));
                                continue;
                            }
                        }
                    }
                    "method_reference" => {
                        let parts = named_children(node);
                        if let [object, name] = parts.as_slice()
                            && text_of(&parsed.source, *name) == setter
                            && self
                                .expression_class(
                                    &method,
                                    text_of(&parsed.source, *object),
                                    node.start_byte(),
                                )?
                                .is_some_and(|owner| owner.id == class.id)
                        {
                            let call = node.parent().and_then(|args| args.parent());
                            if let Some(call) =
                                call.filter(|call| call.kind() == "method_invocation")
                                && invocation(&parsed.source, call).name == "ifPresent"
                                && self.standard_pipeline(&method, call, &parsed.source)?
                            {
                                expression = call.child_by_field_name("object");
                                receiver = Some(text_of(&parsed.source, *object).to_owned());
                            } else {
                                domains.push(unknown(format!(
                                    "unbound setter callback:{}",
                                    text_of(&parsed.source, node)
                                )));
                            }
                        }
                    }
                    _ => {}
                }
                let Some(expression) = expression else {
                    continue;
                };
                proven_sites.insert(node.start_position().row + 1);
                if receiver.as_deref().is_some_and(|receiver| {
                    unused_receiver(&parsed.source, declaration, class, receiver, &setter)
                }) {
                    continue;
                }
                if let Some(receiver) = receiver
                    && let Some(returned) = returned_local(&parsed.source, declaration, class)
                    && receiver != returned
                {
                    let expected = self.copied_field_origins(
                        operation,
                        &method,
                        &returned,
                        declaration.end_byte().saturating_sub(1),
                        reachable,
                    )?;
                    let actual = self.copied_field_origins(
                        operation,
                        &method,
                        &receiver,
                        node.start_byte(),
                        reachable,
                    )?;
                    if !expected.is_empty() && !actual.is_empty() && expected.is_disjoint(&actual) {
                        continue;
                    }
                }
                let mut domain = self.analyze_expression(
                    operation,
                    &method,
                    expression_from_node(&parsed.source, expression),
                    expression.start_byte(),
                    reachable,
                    &mut BTreeSet::new(),
                )?;
                push_unique(
                    &mut domain.evidence,
                    format!(
                        "write:{}:{}:{}",
                        method.file_path,
                        node.start_position().row + 1,
                        text_of(&parsed.source, expression)
                    ),
                );
                push_unique(
                    &mut domain.evidence,
                    format!(
                        "chain:{}",
                        render_path(self.project.graph(), reachable, &method.id)
                    ),
                );
                domains.push(domain);
            }
            for unresolved in self
                .project
                .graph()
                .unresolved(&method.id)
                .iter()
                .filter(|site| site.name == setter && !proven_sites.contains(&site.line))
            {
                domains.push(unknown(format!(
                    "unresolved setter call:{}:{}:{}",
                    unresolved.file_path, unresolved.line, unresolved.column
                )));
            }
        }
        domains.extend(self.field_initializer_domains(
            operation,
            class,
            field,
            reachable,
            &mut BTreeSet::new(),
        )?);
        Ok(domains)
    }

    pub(super) fn field_initializer_domains(
        &mut self,
        operation: &Operation,
        class: &GraphNode,
        field: &str,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, usize)>,
    ) -> Result<Vec<Domain>> {
        let class_source = self.parsed(&class.file_path)?.clone();
        let Some(class_declaration) = type_declaration(&class_source, class) else {
            return Ok(Vec::new());
        };
        let mut domains = Vec::new();
        // Initializers remain possible on branches that never execute an observed write.
        for declaration in primary::owned_nodes(class_declaration, "field_declaration") {
            for variable in named_children(declaration)
                .into_iter()
                .filter(|node| node.kind() == "variable_declarator")
            {
                if variable
                    .child_by_field_name("name")
                    .is_none_or(|name| text_of(&class_source.source, name) != field)
                {
                    continue;
                }
                if let Some(value) = variable.child_by_field_name("value") {
                    domains.push(self.analyze_expression(
                        operation,
                        class,
                        expression_from_node(&class_source.source, value),
                        value.start_byte(),
                        reachable,
                        visiting,
                    )?);
                }
            }
        }
        Ok(domains)
    }

    pub(super) fn typed_setter_edges(
        &mut self,
        class: &GraphNode,
        field: &str,
        methods: &BTreeSet<String>,
    ) -> Result<Vec<GraphEdge>> {
        let name = format!("set{}", uppercase_first(field));
        let mut edges = Vec::new();
        for id in methods {
            let Some(method) = self.project.graph().nodes.get(id).cloned() else {
                continue;
            };
            for site in self.method_invocations(&method)? {
                if site.name != name || !site.exact_arity || site.arguments.len() != 1 {
                    continue;
                }
                let receiver = site.receiver.as_deref().unwrap_or("this");
                if self
                    .expression_class(&method, receiver, site.offset)?
                    .is_some_and(|owner| owner.id == class.id)
                {
                    edges.push(GraphEdge {
                        source: id.clone(),
                        target: class.id.clone(),
                        kind: "calls".into(),
                        line: site.line,
                        column: site.column,
                        metadata: String::new(),
                        provenance: String::new(),
                    });
                }
            }
        }
        Ok(edges)
    }

    #[allow(clippy::too_many_arguments)]
    fn framework_copy_domain(
        &mut self,
        operation: &Operation,
        method: &GraphNode,
        class: &GraphNode,
        field: &str,
        site: &InvocationSite,
        reachable: &Reachability,
    ) -> Result<Option<Domain>> {
        let mut source_field = field.to_owned();
        let source = if site.name == "copyProperties" && site.arguments.len() >= 2 {
            let Some(receiver) = site.receiver.as_deref() else {
                return Ok(None);
            };
            let imported = self.project.imported_type(&method.file_path, receiver);
            let pair = match imported.as_deref() {
                Some("org.springframework.beans.BeanUtils") => (0, 1),
                Some("org.apache.commons.beanutils.BeanUtils") => (1, 0),
                _ => return Ok(None),
            };
            if self
                .expression_class(method, receiver, site.offset)?
                .is_some()
            {
                return Ok(None);
            }
            let Expression::Identifier(target) = &site.arguments[pair.1] else {
                return Ok(None);
            };
            if self
                .expression_class(method, target, site.offset)?
                .is_none_or(|owner| owner.id != class.id)
            {
                return Ok(None);
            }
            for ignore in site.arguments.iter().skip(2) {
                match ignore {
                    Expression::Literal(name)
                        if serde_json::from_str::<String>(name).is_ok_and(|name| name == field) =>
                    {
                        return Ok(None);
                    }
                    Expression::Literal(name) if serde_json::from_str::<String>(name).is_ok() => {}
                    _ => {
                        return Ok(Some(unknown(
                            "BeanUtils ignore list or editable class is not statically proven",
                        )));
                    }
                }
            }
            site.arguments[pair.0].clone()
        } else {
            let targets = self.resolve_invocation(method, site)?;
            let Some(target) = targets
                .first()
                .filter(|_| targets.len() == 1)
                .and_then(|id| self.project.graph().nodes.get(id))
                .cloned()
            else {
                return Ok(None);
            };
            let Some(owner) = self.lexical_owners(&target).first().copied().cloned() else {
                return Ok(None);
            };
            let parsed = self.parsed(&owner.file_path)?.clone();
            let Some(owner_declaration) = type_declaration(&parsed, &owner) else {
                return Ok(None);
            };
            if !primary::has_annotation(&parsed.source, owner_declaration, "Mapper")
                || self
                    .project
                    .imported_type(&owner.file_path, "Mapper")
                    .as_deref()
                    != Some("org.mapstruct.Mapper")
                || parse_java_type(&target.return_type)
                    .and_then(|kind| {
                        self.project
                            .resolve_type(&target.file_path, &owner.qualified_name, &kind)
                    })
                    .is_none_or(|result| result.id != class.id)
            {
                return Ok(None);
            }
            if self.implementation_method(&target).is_some() {
                return Ok(None);
            }
            if site.arguments.len() != 1 {
                return Ok(Some(unknown(
                    "MapStruct multiple source mapping is not proven",
                )));
            }
            let Some(declaration) = lookup::method_declaration(&parsed, &target) else {
                return Ok(None);
            };
            if declaration.child_by_field_name("body").is_some() {
                return Ok(None);
            }
            if primary::has_annotation(&parsed.source, declaration, "BeanMapping")
                || descendants(owner_declaration).into_iter().any(|node| {
                    ["AfterMapping", "BeforeMapping", "ObjectFactory"]
                        .iter()
                        .any(|annotation| primary::has_annotation(&parsed.source, node, annotation))
                })
                || named_children(owner_declaration)
                    .into_iter()
                    .filter(|node| node.kind() == "modifiers")
                    .flat_map(descendants)
                    .any(|node| {
                        node.kind() == "element_value_pair"
                            && node.child_by_field_name("key").is_some_and(|key| {
                                matches!(text_of(&parsed.source, key), "uses" | "config")
                            })
                    })
            {
                return Ok(Some(unknown(
                    "MapStruct lifecycle or mapper configuration is not statically proven",
                )));
            }
            for annotation in descendants(declaration)
                .into_iter()
                .filter(|node| node.kind() == "annotation")
            {
                let name = annotation
                    .child_by_field_name("name")
                    .map(|node| text_of(&parsed.source, node))
                    .unwrap_or_default();
                if name.rsplit('.').next() != Some("Mapping") {
                    continue;
                }
                let mut attributes = HashMap::new();
                for pair in descendants(annotation)
                    .into_iter()
                    .filter(|node| node.kind() == "element_value_pair")
                {
                    if let (Some(key), Some(value)) = (
                        pair.child_by_field_name("key"),
                        pair.child_by_field_name("value"),
                    ) {
                        attributes
                            .insert(text_of(&parsed.source, key), text_of(&parsed.source, value));
                    }
                }
                if attributes
                    .get("target")
                    .and_then(|value| serde_json::from_str::<String>(value).ok())
                    .as_deref()
                    != Some(field)
                {
                    continue;
                }
                if attributes.get("ignore") == Some(&"true") {
                    return Ok(None);
                }
                if attributes
                    .keys()
                    .any(|key| !matches!(*key, "target" | "source" | "ignore"))
                {
                    return Ok(Some(unknown(
                        "MapStruct custom conversion is not statically proven",
                    )));
                }
                if let Some(name) = attributes
                    .get("source")
                    .and_then(|value| serde_json::from_str::<String>(value).ok())
                {
                    source_field = name;
                }
            }
            if source_field.contains('.') {
                return Ok(Some(unknown(
                    "MapStruct nested source mapping is not proven",
                )));
            }
            site.arguments[0].clone()
        };
        let Expression::Identifier(receiver) = source else {
            return Ok(Some(unknown("copy source object identity is not proven")));
        };
        let source_class = self.expression_class(method, &receiver, site.offset)?;
        let field_type = |owner: &GraphNode, name: &str| {
            self.project
                .graph()
                .contained(&owner.id, "field")
                .into_iter()
                .find(|field| field.name == name)
                .and_then(|field| declared_variable_type(&field.signature, &field.name))
                .and_then(|kind| parse_java_type(&kind))
        };
        if let Some(source_type) = source_class
            .as_ref()
            .and_then(|owner| field_type(owner, &source_field))
            && let Some(target_type) = field_type(class, field)
            && !same_copy_type(&source_type, &target_type)
        {
            return Ok(Some(unknown(
                "copy field type conversion is not statically proven",
            )));
        }
        let domain = self.copied_field_domain(
            operation,
            method,
            &receiver,
            &format!("get{}", uppercase_first(&source_field)),
            site.offset,
            reachable,
            &mut BTreeSet::new(),
        )?;
        let mut domain = domain.unwrap_or_else(|| {
            unknown(format!(
                "copy source field is unresolved:{receiver}.{source_field}"
            ))
        });
        domain.evidence.push(format!(
            "copy-framework:{}:{}:{}:{source_field}",
            method.file_path, site.line, site.name
        ));
        Ok(Some(domain))
    }

    fn explicit_builder_value<'n>(
        &mut self,
        method: &GraphNode,
        class: &GraphNode,
        field: &str,
        build: Node<'n>,
        source: &str,
    ) -> Result<Option<Node<'n>>> {
        let mut chain = Vec::new();
        let mut node = build.child_by_field_name("object");
        while let Some(call) = node.filter(|call| call.kind() == "method_invocation") {
            chain.push(call);
            node = call.child_by_field_name("object");
        }
        let Some(factory) = chain.last().copied() else {
            return Ok(None);
        };
        let targets = self.resolve_invocation(method, &invocation(source, factory))?;
        let Some(factory_method) = targets
            .first()
            .and_then(|id| self.project.graph().nodes.get(id))
            .cloned()
        else {
            return Ok(None);
        };
        let factory_source = self.parsed(&factory_method.file_path)?.clone();
        let Some(factory_declaration) =
            lookup::method_declaration(&factory_source, &factory_method)
        else {
            return Ok(None);
        };
        let Some(creation) = single_return(factory_declaration) else {
            return Ok(None);
        };
        if creation.kind() != "object_creation_expression" {
            return Ok(None);
        }
        let Some(builder_class) = creation
            .child_by_field_name("type")
            .and_then(|kind| parse_java_type(text_of(&factory_source.source, kind)))
            .and_then(|kind| {
                self.project.resolve_type(
                    &factory_method.file_path,
                    &factory_method.qualified_name,
                    &kind,
                )
            })
            .cloned()
        else {
            return Ok(None);
        };
        let builds = self
            .project
            .graph()
            .contained(&builder_class.id, "method")
            .into_iter()
            .filter(|method| {
                method.name == "build" && method_parameters(&method.signature).is_empty()
            })
            .cloned()
            .collect::<Vec<_>>();
        let [build_method] = builds.as_slice() else {
            return Ok(None);
        };
        let parsed = self.parsed(&build_method.file_path)?.clone();
        let Some(build_declaration) = lookup::method_declaration(&parsed, build_method) else {
            return Ok(None);
        };
        let Some(creation) = single_return(build_declaration)
            .filter(|node| node.kind() == "object_creation_expression")
        else {
            return Ok(None);
        };
        if creation
            .child_by_field_name("type")
            .and_then(|kind| parse_java_type(text_of(&parsed.source, kind)))
            .and_then(|kind| {
                self.project.resolve_type(
                    &build_method.file_path,
                    &build_method.qualified_name,
                    &kind,
                )
            })
            .is_none_or(|owner| owner.id != class.id)
        {
            return Ok(None);
        }
        let args = creation
            .child_by_field_name("arguments")
            .map(named_children)
            .unwrap_or_default();
        let class_source = self.parsed(&class.file_path)?.clone();
        let Some(declaration) = type_declaration(&class_source, class) else {
            return Ok(None);
        };
        let Some(index) = primary::argument_index(
            &class_source.source,
            declaration,
            field,
            &primary::instance_fields(&class_source.source, declaration),
            args.len(),
        ) else {
            return Ok(None);
        };
        let Some(stored) = args
            .get(index)
            .filter(|node| matches!(node.kind(), "identifier" | "field_access"))
        else {
            return Ok(None);
        };
        let stored = text_of(&parsed.source, *stored).trim_start_matches("this.");
        for call in chain {
            let site = invocation(source, call);
            if site.arguments.len() != 1 {
                continue;
            }
            let setters = self
                .project
                .graph()
                .contained(&builder_class.id, "method")
                .into_iter()
                .filter(|method| {
                    method.name == site.name && method_parameters(&method.signature).len() == 1
                })
                .cloned()
                .collect::<Vec<_>>();
            let [setter] = setters.as_slice() else {
                return Ok(None);
            };
            let setter_source = self.parsed(&setter.file_path)?.clone();
            let Some(setter_declaration) = lookup::method_declaration(&setter_source, setter)
            else {
                return Ok(None);
            };
            let Some(body) = setter_declaration.child_by_field_name("body") else {
                return Ok(None);
            };
            let statements = lookup::statements(body);
            if statements.len() != 2
                || statements[0].kind() != "expression_statement"
                || statements[1].kind() != "return_statement"
                || statements[1]
                    .named_child(0)
                    .is_none_or(|node| text_of(&setter_source.source, node) != "this")
            {
                return Ok(None);
            }
            let Some(assignment) = statements[0]
                .named_child(0)
                .filter(|node| node.kind() == "assignment_expression")
            else {
                return Ok(None);
            };
            if assignment.child_by_field_name("left").is_some_and(|node| {
                text_of(&setter_source.source, node) == format!("this.{stored}")
            }) {
                let parameter = &method_parameters(&setter.signature)[0].1;
                if assignment
                    .child_by_field_name("right")
                    .is_some_and(|node| text_of(&setter_source.source, node) == parameter)
                    && assignment
                        .child_by_field_name("operator")
                        .is_some_and(|node| text_of(&setter_source.source, node) == "=")
                {
                    return Ok(call
                        .child_by_field_name("arguments")
                        .and_then(|args| args.named_child(0)));
                }
                return Ok(None);
            }
        }
        Ok(None)
    }
}

fn single_return(declaration: Node<'_>) -> Option<Node<'_>> {
    let statements = lookup::statements(declaration.child_by_field_name("body")?);
    if statements.len() != 1 || statements[0].kind() != "return_statement" {
        return None;
    }
    statements[0].named_child(0)
}

fn same_copy_type(left: &TypeRef, right: &TypeRef) -> bool {
    let primitive = |kind: &TypeRef| match kind.simple_name() {
        "Integer" | "int" => "int",
        "Long" | "long" => "long",
        "Short" | "short" => "short",
        "Byte" | "byte" => "byte",
        "Double" | "double" => "double",
        "Float" | "float" => "float",
        "Character" | "char" => "char",
        "Boolean" | "boolean" => "boolean",
        _ => "",
    };
    (left.simple_name() == right.simple_name()
        || (!primitive(left).is_empty() && primitive(left) == primitive(right)))
        && left.array_depth == right.array_depth
        && left.arguments.len() == right.arguments.len()
        && left
            .arguments
            .iter()
            .zip(&right.arguments)
            .all(|(left, right)| same_copy_type(left, right))
}

fn has_definite_write(
    source: &str,
    method: Node<'_>,
    creation: Node<'_>,
    field: &str,
    setter: &str,
) -> bool {
    let Some(variable) = creation
        .parent()
        .filter(|node| node.kind() == "variable_declarator")
    else {
        return false;
    };
    let Some(name) = variable.child_by_field_name("name") else {
        return false;
    };
    let name = text_of(source, name);
    descendants(method).into_iter().any(|node| {
        if node.start_byte() <= creation.start_byte() {
            return false;
        }
        if descendants(method).into_iter().any(|earlier| {
            earlier.kind() == "return_statement"
                && earlier.start_byte() > creation.start_byte()
                && earlier.start_byte() < node.start_byte()
        }) {
            return false;
        }
        if lookup::ancestors(node)
            .take_while(|node| *node != method)
            .any(|node| {
                matches!(
                    node.kind(),
                    "if_statement"
                        | "switch_expression"
                        | "for_statement"
                        | "enhanced_for_statement"
                        | "while_statement"
                        | "do_statement"
                        | "lambda_expression"
                        | "catch_clause"
                )
            })
        {
            return false;
        }
        (node.kind() == "method_invocation"
            && node
                .child_by_field_name("name")
                .is_some_and(|node| text_of(source, node) == setter)
            && node
                .child_by_field_name("object")
                .is_some_and(|node| text_of(source, node) == name))
            || (node.kind() == "assignment_expression"
                && node
                    .child_by_field_name("left")
                    .is_some_and(|node| text_of(source, node) == format!("{name}.{field}")))
    })
}

fn returned_local(source: &str, method: Node<'_>, class: &GraphNode) -> Option<String> {
    if method
        .child_by_field_name("type")
        .is_none_or(|kind| text_of(source, kind) != class.name)
    {
        return None;
    }
    let returns = descendants(method)
        .into_iter()
        .filter(|node| node.kind() == "return_statement")
        .map(|node| named_children(node).into_iter().next())
        .collect::<Option<Vec<_>>>()?;
    if returns.is_empty() || returns.iter().any(|node| node.kind() != "identifier") {
        return None;
    }
    let names = returns
        .into_iter()
        .map(|node| text_of(source, node))
        .collect::<BTreeSet<_>>();
    (names.len() == 1).then(|| names.into_iter().next().unwrap().to_owned())
}

fn unused_receiver(
    source: &str,
    method: Node<'_>,
    class: &GraphNode,
    receiver: &str,
    setter: &str,
) -> bool {
    if method
        .child_by_field_name("type")
        .and_then(|kind| parse_java_type(text_of(source, kind)))
        .is_none_or(|kind| kind.simple_name() != class.name)
    {
        return false;
    }
    let identifiers = descendants(method)
        .into_iter()
        .filter(|node| node.kind() == "identifier" && text_of(source, *node) == receiver)
        .collect::<Vec<_>>();
    let local = identifiers.iter().any(|node| {
        node.parent().is_some_and(|parent| {
            parent.kind() == "variable_declarator"
                && parent.child_by_field_name("name") == Some(*node)
                && parent
                    .child_by_field_name("value")
                    .is_some_and(|value| value.kind() == "object_creation_expression")
        })
    });
    local
        && identifiers.iter().all(|node| {
            node.parent().is_some_and(|parent| {
                (parent.kind() == "variable_declarator"
                    && parent.child_by_field_name("name") == Some(*node))
                    || (parent.kind() == "method_invocation"
                        && parent.child_by_field_name("object") == Some(*node)
                        && invocation(source, parent).name == setter
                        && parent
                            .parent()
                            .is_some_and(|statement| statement.kind() == "expression_statement"))
                    || (parent.kind() == "field_access"
                        && parent.child_by_field_name("object") == Some(*node)
                        && parent.parent().is_some_and(|assignment| {
                            assignment.kind() == "assignment_expression"
                                && assignment.child_by_field_name("left") == Some(parent)
                        }))
            })
        })
}
