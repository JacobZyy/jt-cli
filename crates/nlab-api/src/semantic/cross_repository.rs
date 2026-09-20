use super::*;

#[cfg(test)]
mod tests;

impl SemanticAnalyzer<'_> {
    pub(super) fn lookup_field_domain(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        accessor: &str,
        offset: usize,
    ) -> Result<Option<Domain>> {
        let Some(origin) = self.getter_origin(
            method,
            Expression::Getter {
                receiver: receiver.to_owned(),
                accessor: accessor.to_owned(),
            },
            offset,
            0,
        )?
        else {
            return Ok(None);
        };
        let mut domain = Domain::default();
        for invocation in self.method_invocations(method)? {
            for (index, argument) in invocation.arguments.iter().enumerate() {
                let Some(candidate) =
                    self.getter_origin(method, argument.clone(), invocation.offset, 0)?
                else {
                    continue;
                };
                if origin.0 != candidate.0
                    || origin.1 != candidate.1
                    || self.getter_changed_between(
                        method,
                        &origin.0,
                        &origin.1,
                        origin.2,
                        candidate.2,
                    )?
                {
                    continue;
                }
                let targets = self.resolve_invocation(method, &invocation)?;
                let Some(target) = targets
                    .first()
                    .filter(|_| targets.len() == 1)
                    .and_then(|id| self.project.graph().nodes.get(id))
                    .cloned()
                else {
                    continue;
                };
                let key = (target.id.clone(), index);
                let lookup = if let Some(domain) = self.lookup_forwarder_cache.get(&key) {
                    domain.clone()
                } else {
                    let domain =
                        self.forwarded_lookup_domain(&target, index, &mut BTreeSet::new())?;
                    self.lookup_forwarder_cache.insert(key, domain.clone());
                    domain
                };
                let Some(lookup) = lookup else {
                    continue;
                };
                merge_domain(&mut domain, lookup);
                push_unique(
                    &mut domain.evidence,
                    format!(
                        "field-lookup:{}:{}:{}({receiver}.{accessor})",
                        method.file_path, invocation.line, invocation.name
                    ),
                );
            }
        }
        if domain.enum_fqn.is_none() {
            return Ok(None);
        }
        domain.closure_gaps.insert("enum reverse lookup identifies known values but does not constrain every returned value".to_owned());
        Ok(Some(domain))
    }

    /// Match the value being returned, not a getter name on an unrelated object.
    fn getter_origin(
        &mut self,
        method: &GraphNode,
        expression: Expression,
        offset: usize,
        depth: usize,
    ) -> Result<Option<(String, String, usize)>> {
        if depth >= 16 {
            return Ok(None);
        }
        match expression {
            Expression::Identifier(name) => match self.local_value(method, &name, offset)? {
                Some((value, position)) => self.getter_origin(method, value, position, depth + 1),
                None => Ok(None),
            },
            Expression::Getter {
                mut receiver,
                accessor,
            } => {
                for _ in depth..16 {
                    if !receiver.split('.').all(|part| {
                        !part.is_empty()
                            && part
                                .chars()
                                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
                    }) {
                        return Ok(None);
                    }
                    match self.local_value(method, &receiver, offset)? {
                        Some((Expression::Identifier(alias), _)) if alias != receiver => {
                            receiver = alias
                        }
                        Some((Expression::Unknown(_), _)) => return Ok(None),
                        _ => return Ok(Some((receiver, accessor, offset))),
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn getter_changed_between(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        accessor: &str,
        first: usize,
        second: usize,
    ) -> Result<bool> {
        let (start, end) = (first.min(second), first.max(second));
        let setter = format!(
            "set{}",
            uppercase_first(getter_signal(accessor).as_deref().unwrap_or(accessor))
        );
        for invocation in self.method_invocations(method)? {
            if invocation.offset > start && invocation.offset < end && invocation.name == setter {
                if let Some(object) = invocation.receiver {
                    let expression = Expression::Getter {
                        receiver: object,
                        accessor: accessor.to_owned(),
                    };
                    if self
                        .getter_origin(method, expression, invocation.offset, 0)?
                        .is_some_and(|origin| origin.0 == receiver)
                    {
                        return Ok(true);
                    }
                }
            }
        }
        let parsed = self.parsed(&method.file_path)?;
        let Some(declaration) = lookup::method_declaration(parsed, method) else {
            return Ok(true);
        };
        let changes = descendants(declaration)
            .iter()
            .filter(|node| node.start_byte() > start && node.start_byte() < end)
            .filter_map(|node| match node.kind() {
                "assignment_expression" => node.child_by_field_name("left"),
                "update_expression" => node.named_child(0),
                _ => None,
            })
            .map(|left| (text_of(&parsed.source, left).to_owned(), left.start_byte()))
            .collect::<Vec<_>>();
        for (left, position) in changes {
            if left == receiver {
                return Ok(true);
            }
            if let Some((object, field)) = left.rsplit_once('.') {
                if Some(field) == getter_signal(accessor).as_deref() {
                    let expression = Expression::Getter {
                        receiver: object.to_owned(),
                        accessor: accessor.to_owned(),
                    };
                    if self
                        .getter_origin(method, expression, position, 0)?
                        .is_some_and(|origin| origin.0 == receiver)
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn forwarded_lookup_domain(
        &mut self,
        method: &GraphNode,
        index: usize,
        visiting: &mut BTreeSet<(String, usize)>,
    ) -> Result<Option<Domain>> {
        if index == 0 {
            if let Some(lookup) = self.enum_lookups.get(&method.id) {
                return Ok(Some(lookup.domain.clone()));
            }
        }
        let key = (method.id.clone(), index);
        if visiting.len() >= 16 || !visiting.insert(key.clone()) {
            return Ok(None);
        }
        let parameters = method_parameters(&method.signature);
        let Some((_, parameter)) = parameters.get(index) else {
            visiting.remove(&key);
            return Ok(None);
        };
        let mut domain = Domain::default();
        for invocation in self.method_invocations(method)? {
            let targets = self.resolve_invocation(method, &invocation)?;
            let Some(target) = targets
                .first()
                .filter(|_| targets.len() == 1)
                .and_then(|id| self.project.graph().nodes.get(id))
                .cloned()
            else {
                continue;
            };
            for (argument_index, argument) in invocation.arguments.iter().enumerate() {
                if !matches!(argument, Expression::Identifier(name) if name == parameter) {
                    continue;
                }
                if let Some(lookup) =
                    self.forwarded_lookup_domain(&target, argument_index, visiting)?
                {
                    merge_domain(&mut domain, lookup);
                }
            }
        }
        visiting.remove(&key);
        Ok(domain.enum_fqn.is_some().then_some(domain))
    }

    pub(super) fn invocation_reaches(
        &mut self,
        caller: &GraphNode,
        invocation: &InvocationSite,
        method_id: &str,
    ) -> Result<bool> {
        let Some(actual) = self.project.graph().nodes.get(method_id).cloned() else {
            return Ok(false);
        };
        for id in self.resolve_invocation(caller, invocation)? {
            if id == method_id {
                return Ok(true);
            }
            let Some(declared) = self.project.graph().nodes.get(&id).cloned() else {
                continue;
            };
            if self.implementation_method(&declared).as_deref() == Some(method_id) {
                return Ok(true);
            }
            if declared.name != actual.name
                || method_parameters(&declared.signature)
                    .iter()
                    .map(|(kind, _)| kind)
                    .collect::<Vec<_>>()
                    != method_parameters(&actual.signature)
                        .iter()
                        .map(|(kind, _)| kind)
                        .collect::<Vec<_>>()
            {
                continue;
            }
            let parsed = self.parsed(&declared.file_path)?;
            if !lookup::method_declaration(parsed, &declared)
                .is_some_and(|node| node.child_by_field_name("body").is_none())
            {
                continue;
            }
            let graph = self.project.graph();
            let owner = |method: &str| {
                graph
                    .edges
                    .iter()
                    .find(|edge| edge.kind == "contains" && edge.target == method)
                    .map(|edge| edge.source.clone())
            };
            let (Some(expected), Some(owner)) = (owner(&declared.id), owner(&actual.id)) else {
                continue;
            };
            let mut queue = vec![owner];
            let mut visited = BTreeSet::new();
            while let Some(owner) = queue.pop() {
                if !visited.insert(owner.clone()) {
                    continue;
                }
                if owner == expected {
                    return Ok(true);
                }
                queue.extend(
                    graph
                        .outgoing(&owner)
                        .filter(|edge| matches!(edge.kind.as_str(), "extends" | "implements"))
                        .map(|edge| edge.target.clone()),
                );
            }
        }
        Ok(false)
    }

    /// Associate DTO copies only when their concrete source objects can be traced.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn copied_field_domain(
        &mut self,
        operation: &Operation,
        writer: &GraphNode,
        receiver: &str,
        accessor: &str,
        offset: usize,
        reachable: &Reachability,
        visiting: &mut BTreeSet<(String, usize)>,
    ) -> Result<Option<Domain>> {
        let Some(field_name) = getter_signal(accessor) else {
            return Ok(None);
        };
        let Some(type_name) =
            self.receiver_type(writer, receiver.trim_start_matches("this."), offset)?
        else {
            return Ok(None);
        };
        let Some(type_ref) = parse_java_type(&type_name) else {
            return Ok(None);
        };
        let Some(class) = self
            .project
            .resolve_type(&writer.file_path, &writer.qualified_name, &type_ref)
            .cloned()
        else {
            return Ok(None);
        };
        let cache_key = (
            operation.key.clone(),
            format!("{}:{}:{receiver}:{offset}", class.id, writer.id),
            field_name.clone(),
        );
        if let Some(domain) = self.field_domain_cache.get(&cache_key) {
            return Ok(Some(domain.clone()));
        }
        let visit_key = (format!("field:{}:{field_name}", class.id), 0);
        if visiting.len() >= 32 || !visiting.insert(visit_key.clone()) {
            return Ok(None);
        }
        let mut domain = Domain::default();
        let setter_name = format!("set{}", uppercase_first(&field_name));
        let setters = self
            .project
            .graph()
            .contained(&class.id, "method")
            .into_iter()
            .filter(|method| method.name == setter_name)
            .cloned()
            .collect::<Vec<_>>();
        if setters.len() == 1 {
            let setter = &setters[0];
            let edges = self
                .project
                .graph()
                .incoming_calls(&setter.id)
                .filter(|edge| reachable.nodes.contains(&edge.source))
                .cloned()
                .collect::<Vec<_>>();
            let mut proven_edges = Vec::new();
            for edge in &edges {
                if self.copied_setter_edge_matches(
                    operation,
                    writer,
                    receiver,
                    offset,
                    reachable,
                    edge,
                    &setter.name,
                )? {
                    proven_edges.push(edge.clone());
                }
            }
            let source_proven = !proven_edges.is_empty();
            let edges = if source_proven { proven_edges } else { edges };
            for edge in edges {
                let source = self.project.graph().nodes[&edge.source].clone();
                let Some((expression, _, value_offset)) =
                    self.setter_argument(&edge, &setter.name)?
                else {
                    domain
                        .unknown
                        .insert("copied field has an unresolved assignment".to_owned());
                    continue;
                };
                let value = self.analyze_expression(
                    operation,
                    &source,
                    expression,
                    value_offset,
                    reachable,
                    visiting,
                )?;
                merge_domain(&mut domain, value);
                push_unique(
                    &mut domain.evidence,
                    format!(
                        "copy-source:{}:{}:{}",
                        source.file_path, edge.line, field_name
                    ),
                );
            }
            if !source_proven {
                domain
                    .unknown
                    .insert("copied field object origin is not proven".to_owned());
            }
        }
        visiting.remove(&visit_key);
        let documented = self.copied_field_enum_reference(&class, &field_name, &mut domain);
        if domain.enum_fqn.is_none() && !documented {
            return Ok(None);
        }
        if domain.enum_fqn.is_some() {
            domain.closure_gaps.insert(
                "DTO copy links an enum source but does not prove a closed field domain".to_owned(),
            );
        }
        push_unique(
            &mut domain.evidence,
            format!(
                "copy:{}:{}:{}#{}",
                writer.file_path, offset, class.qualified_name, field_name
            ),
        );
        self.field_domain_cache.insert(cache_key, domain.clone());
        Ok(Some(domain))
    }

    fn copied_field_enum_reference(
        &self,
        class: &GraphNode,
        field_name: &str,
        domain: &mut Domain,
    ) -> bool {
        let Some(field) = self
            .project
            .graph()
            .contained(&class.id, "field")
            .into_iter()
            .find(|field| field.name == field_name)
        else {
            return false;
        };
        let Some(description) = field.docstring.as_deref() else {
            return false;
        };
        let references = see_enum_references(description);
        if references.is_empty() {
            return false;
        }
        let candidates = linked_enum_nodes(
            self.project,
            &field.file_path,
            &class.qualified_name,
            description,
        );
        for candidate in &candidates {
            let name = candidate.qualified_name.replace("::", ".");
            let status = match domain.enum_fqn.as_deref() {
                Some(actual) if actual == name => "corroborated",
                Some(_) => "conflicts-with-code",
                None => "unverified",
            };
            push_unique(
                &mut domain.evidence,
                format!(
                    "enum-reference:{}:{}:{name}:{status}",
                    field.file_path, field.start_line
                ),
            );
        }
        if domain.enum_fqn.is_none() {
            let reason = if candidates.len() == 1 {
                "unverified"
            } else {
                "unresolved or ambiguous"
            };
            domain.unknown.insert(format!(
                "{reason} @see enum reference: {}",
                references.join(", ")
            ));
        }
        true
    }
}
