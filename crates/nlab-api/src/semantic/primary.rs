use super::*;

#[cfg(test)]
mod tests;

pub(super) fn instance_fields(source: &str, declaration: Node<'_>) -> Vec<String> {
    owned_nodes(declaration, "field_declaration")
        .into_iter()
        .filter(|field| !has_modifier(source, *field, "static"))
        .flat_map(named_children)
        .filter(|node| node.kind() == "variable_declarator")
        .filter_map(|node| node.child_by_field_name("name"))
        .map(|name| text_of(source, name).to_owned())
        .collect()
}

pub(super) fn field_name(source: &str, declaration: Node<'_>, fields: &[String]) -> Option<String> {
    let methods = owned_nodes(declaration, "method_declaration");
    let annotated = owned_nodes(declaration, "field_declaration")
        .into_iter()
        .filter(|field| has_annotation(source, *field, "JsonValue"))
        .flat_map(named_children)
        .filter(|node| node.kind() == "variable_declarator")
        .filter_map(|node| node.child_by_field_name("name"))
        .map(|node| text_of(source, node).to_owned())
        .collect::<BTreeSet<_>>();
    let json_methods = methods
        .iter()
        .filter(|method| has_annotation(source, **method, "JsonValue"))
        .copied()
        .collect::<Vec<_>>();
    if !annotated.is_empty() || !json_methods.is_empty() {
        let mut candidates = annotated;
        for method in json_methods {
            candidates.insert(returned_field(source, method, fields)?);
        }
        return unique(candidates);
    }
    let value_methods = methods
        .iter()
        .filter(|method| method_name(source, **method) == "val")
        .copied()
        .collect::<Vec<_>>();
    if !value_methods.is_empty() {
        return unique(
            value_methods
                .into_iter()
                .map(|method| returned_field(source, method, fields))
                .collect::<Option<BTreeSet<_>>>()?,
        );
    }

    let enum_name = text_of(source, declaration.child_by_field_name("name")?);
    let identity_fields = fields
        .iter()
        .filter(|field| identity_name(enum_name, field))
        .cloned()
        .collect::<BTreeSet<_>>();
    let lookup_fields = methods
        .into_iter()
        .filter(|method| has_modifier(source, *method, "static"))
        .filter(|method| {
            method
                .child_by_field_name("type")
                .is_some_and(|node| text_of(source, node) == enum_name)
        })
        .filter_map(|method| lookup::lookup_projection(source, method, enum_name))
        .filter_map(|(accessor, _)| accessor_field(source, declaration, &accessor, fields))
        .filter(|field| identity_fields.contains(field))
        .collect::<BTreeSet<_>>();
    if !lookup_fields.is_empty() {
        return unique(lookup_fields);
    }
    unique(identity_fields)
}

pub(super) fn accessor_field(
    source: &str,
    declaration: Node<'_>,
    accessor: &str,
    fields: &[String],
) -> Option<String> {
    if fields.iter().any(|field| field == accessor) {
        return Some(accessor.to_owned());
    }
    let methods = owned_nodes(declaration, "method_declaration")
        .into_iter()
        .filter(|method| method_name(source, *method) == accessor)
        .collect::<Vec<_>>();
    if let [method] = methods.as_slice() {
        return returned_field(source, *method, fields);
    }
    if !methods.is_empty() {
        return None;
    }
    let field = getter_signal(accessor)?;
    if !fields.contains(&field) {
        return None;
    }
    let getter_generated = ["Getter", "Data", "Value"]
        .iter()
        .any(|annotation| has_annotation(source, declaration, annotation))
        || owned_nodes(declaration, "field_declaration")
            .into_iter()
            .any(|node| {
                has_annotation(source, node, "Getter")
                    && named_children(node).into_iter().any(|variable| {
                        variable.kind() == "variable_declarator"
                            && variable
                                .child_by_field_name("name")
                                .is_some_and(|name| text_of(source, name) == field)
                    })
            });
    getter_generated.then_some(field)
}

pub(super) fn argument_index(
    source: &str,
    declaration: Node<'_>,
    field: &str,
    fields: &[String],
    arity: usize,
) -> Option<usize> {
    let constructors = owned_nodes(declaration, "constructor_declaration");
    if constructors.is_empty() {
        return (has_annotation(source, declaration, "AllArgsConstructor")
            && fields.len() == arity)
            .then(|| fields.iter().position(|candidate| candidate == field))
            .flatten();
    }
    let constructors = constructors
        .into_iter()
        .filter(|constructor| {
            constructor
                .child_by_field_name("parameters")
                .is_some_and(|parameters| named_children(parameters).len() == arity)
        })
        .collect::<Vec<_>>();
    let [constructor] = constructors.as_slice() else {
        return None;
    };
    let body = constructor.child_by_field_name("body")?;
    let statements = named_children(body);
    // Keep constructor analysis bounded to direct field assignments.
    if statements
        .iter()
        .any(|node| node.kind() != "expression_statement")
    {
        return None;
    }
    let expressions = statements
        .into_iter()
        .filter_map(|node| named_children(node).into_iter().next())
        .collect::<Vec<_>>();
    if expressions.iter().any(|node| {
        node.kind() != "assignment_expression"
            || node
                .child_by_field_name("left")
                .is_none_or(|left| !text_of(source, left).starts_with("this."))
            || node
                .child_by_field_name("operator")
                .is_none_or(|operator| text_of(source, operator) != "=")
            || node.child_by_field_name("right").is_none_or(|right| {
                right.kind() != "identifier" && !right.kind().ends_with("_literal")
            })
    }) {
        return None;
    }
    let assignments = expressions
        .into_iter()
        .filter(|node| {
            node.child_by_field_name("left")
                .is_some_and(|left| text_of(source, left) == format!("this.{field}"))
        })
        .collect::<Vec<_>>();
    let [assignment] = assignments.as_slice() else {
        return None;
    };
    if assignment
        .child_by_field_name("operator")
        .is_none_or(|operator| text_of(source, operator) != "=")
    {
        return None;
    }
    let value = assignment.child_by_field_name("right")?;
    if value.kind() != "identifier" {
        return None;
    }
    let parameter = text_of(source, value);
    named_children(constructor.child_by_field_name("parameters")?)
        .iter()
        .position(|node| {
            node.child_by_field_name("name")
                .is_some_and(|name| text_of(source, name) == parameter)
        })
}

fn returned_field(source: &str, method: Node<'_>, fields: &[String]) -> Option<String> {
    if has_modifier(source, method, "static")
        || method
            .child_by_field_name("parameters")
            .is_none_or(|parameters| !named_children(parameters).is_empty())
    {
        return None;
    }
    let statements = named_children(method.child_by_field_name("body")?);
    let [statement] = statements.as_slice() else {
        return None;
    };
    if statement.kind() != "return_statement" {
        return None;
    }
    let expression = named_children(*statement).into_iter().next()?;
    let field = text_of(source, expression)
        .trim()
        .trim_start_matches("this.");
    fields
        .iter()
        .find(|candidate| candidate.as_str() == field)
        .cloned()
}

fn identity_name(enum_name: &str, field: &str) -> bool {
    if matches!(field, "buttonType" | "actions")
        || [
            "name",
            "desc",
            "description",
            "label",
            "title",
            "text",
            "message",
            "color",
            "colour",
            "style",
            "icon",
        ]
        .iter()
        .any(|suffix| field == *suffix || field.ends_with(&uppercase_first(suffix)))
    {
        return false;
    }
    if matches!(
        field,
        "code" | "value" | "state" | "status" | "type" | "id" | "key" | "result" | "channel"
    ) {
        return true;
    }
    let owner = enum_name.strip_suffix("Enum").unwrap_or(enum_name);
    owner.ends_with(&uppercase_first(field))
        || field
            .strip_suffix("Code")
            .is_some_and(|stem| !stem.is_empty() && owner.ends_with(&uppercase_first(stem)))
}

fn unique(values: BTreeSet<String>) -> Option<String> {
    (values.len() == 1)
        .then(|| values.into_iter().next())
        .flatten()
}

fn method_name<'a>(source: &'a str, node: Node<'_>) -> &'a str {
    node.child_by_field_name("name")
        .map(|name| text_of(source, name))
        .unwrap_or("")
}

fn has_modifier(source: &str, node: Node<'_>, expected: &str) -> bool {
    named_children(node)
        .into_iter()
        .filter(|node| node.kind() == "modifiers")
        .any(|modifiers| {
            text_of(source, modifiers)
                .split_whitespace()
                .any(|token| token == expected)
        })
}

fn has_annotation(source: &str, node: Node<'_>, expected: &str) -> bool {
    named_children(node)
        .into_iter()
        .filter(|node| node.kind() == "modifiers")
        .flat_map(named_children)
        .filter(|node| matches!(node.kind(), "annotation" | "marker_annotation"))
        .any(|annotation| {
            if !annotation
                .child_by_field_name("name")
                .is_some_and(|name| text_of(source, name).rsplit('.').next() == Some(expected))
            {
                return false;
            }
            if expected == "JsonValue" {
                let text = text_of(source, annotation)
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect::<String>();
                return !text.contains("(false)") && !text.contains("(value=false)");
            }
            true
        })
}

fn owned_nodes<'a>(declaration: Node<'a>, kind: &str) -> Vec<Node<'a>> {
    descendants(declaration)
        .into_iter()
        .filter(|node| node.kind() == kind)
        .filter(|node| {
            lookup::ancestors(*node).find(|parent| {
                matches!(
                    parent.kind(),
                    "class_declaration" | "enum_declaration" | "interface_declaration"
                )
            }) == Some(declaration)
        })
        .collect()
}
