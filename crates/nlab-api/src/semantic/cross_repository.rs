use super::*;

#[cfg(test)]
mod tests;

impl SemanticAnalyzer<'_> {
    pub(super) fn lookup_field_domain(
        &mut self,
        method: &GraphNode,
        receiver: &str,
        accessor: &str,
    ) -> Result<Option<Domain>> {
        let mut domain = Domain::default();
        for invocation in self.method_invocations(method)? {
            if !matches!(invocation.arguments.as_slice(), [Expression::Getter { receiver: name, accessor: getter }] if name == receiver && getter == accessor)
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
            let lookup = if let Some(domain) = self.lookup_forwarder_cache.get(&target.id) {
                domain.clone()
            } else {
                let domain = self.forwarded_lookup_domain(&target, 0, &mut BTreeSet::new())?;
                self.lookup_forwarder_cache
                    .insert(target.id.clone(), domain.clone());
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
        if domain.enum_fqn.is_none() {
            return Ok(None);
        }
        domain.unknown.insert("enum reverse lookup identifies known values but does not constrain every returned value".to_owned());
        Ok(Some(domain))
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
        Ok(self
            .resolve_invocation(caller, invocation)?
            .iter()
            .any(|id| {
                id == method_id
                    || self
                        .project
                        .graph()
                        .nodes
                        .get(id)
                        .and_then(|method| self.implementation_method(method))
                        .as_deref()
                        == Some(method_id)
            }))
    }

    /// Preserve declaration values across DTO copies, without claiming object identity or full path coverage.
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
        let cache_key = (operation.key.clone(), class.id.clone(), field_name.clone());
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
            for edge in edges {
                let source = self.project.graph().nodes[&edge.source].clone();
                let Some((expression, _, value_offset)) =
                    self.setter_argument(&edge, &setter.name)?
                else {
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
        }
        visiting.remove(&visit_key);
        if domain.enum_fqn.is_none() {
            return Ok(None);
        }
        domain.unknown.insert(
            "DTO copy links an enum source but does not prove a closed field domain".to_owned(),
        );
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
}
