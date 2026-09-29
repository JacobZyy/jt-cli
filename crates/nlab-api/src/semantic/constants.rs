use super::*;

/// Evaluate literals and immutable source-backed aliases, never execute Java code.
pub(super) fn value(
    project: &JavaProject<'_>,
    context: &GraphNode,
    expression: &str,
) -> Result<Option<WireValue>> {
    resolve(project, context, expression.trim(), &mut BTreeSet::new())
}

fn resolve(
    project: &JavaProject<'_>,
    context: &GraphNode,
    expression: &str,
    visiting: &mut BTreeSet<(String, String)>,
) -> Result<Option<WireValue>> {
    if let Ok(value) = serde_json::from_str::<WireValue>(expression) {
        return Ok(Some(value));
    }
    let integer = expression.trim_end_matches(['l', 'L']).replace('_', "");
    if let Ok(value) = integer.parse::<i64>() {
        return Ok(Some(WireValue::Number(value)));
    }
    if visiting.len() >= 24
        || !expression
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '_' | '$'))
        || !visiting.insert((context.id.clone(), expression.to_owned()))
    {
        return Ok(None);
    }
    let (qualifier, name) = expression
        .rsplit_once('.')
        .map_or((None, expression), |(owner, name)| (Some(owner), name));
    let mut owners = Vec::new();
    if let Some(qualifier) = qualifier.filter(|qualifier| *qualifier != "this") {
        if let Some(kind) = parse_java_type(qualifier)
            && let Some(owner) =
                project.resolve_type(&context.file_path, &context.qualified_name, &kind)
        {
            owners.push(owner);
        } else if name == "EMPTY"
            && project
                .imported_type(&context.file_path, qualifier)
                .is_some_and(|name| {
                    matches!(
                        name.as_str(),
                        "org.apache.commons.lang3.StringUtils"
                            | "org.apache.commons.lang.StringUtils"
                    )
                })
        {
            // This library constant has fixed semantics; a local shadowing type wins above.
            return Ok(Some(WireValue::String(String::new())));
        }
    } else {
        let mut scope = context.qualified_name.replace("::", ".");
        loop {
            if let Some(owner) = project.node_for_fqn(&scope) {
                owners.push(owner);
            }
            let Some((parent, _)) = scope.rsplit_once('.') else {
                break;
            };
            scope = parent.to_owned();
        }
    }
    for owner in owners {
        let source = project.source(&owner.file_path)?;
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
        let tree = parser
            .parse(source, None)
            .context("parse constant source")?;
        let Some(declaration) = descendants(tree.root_node()).into_iter().find(|node| {
            matches!(
                node.kind(),
                "class_declaration" | "interface_declaration" | "enum_declaration"
            ) && node.start_position().row + 1 == owner.start_line
                && node
                    .child_by_field_name("name")
                    .is_some_and(|node| text_of(source, node) == owner.name)
        }) else {
            continue;
        };
        let Some(body) = declaration.child_by_field_name("body") else {
            continue;
        };
        for field in named_children(body)
            .into_iter()
            .flat_map(|node| {
                if node.kind() == "enum_body_declarations" {
                    named_children(node)
                } else {
                    vec![node]
                }
            })
            .filter(|node| matches!(node.kind(), "field_declaration" | "constant_declaration"))
        {
            let modifiers = text_of(source, field)
                .split_whitespace()
                .collect::<BTreeSet<_>>();
            if declaration.kind() != "interface_declaration"
                && !(modifiers.contains("static") && modifiers.contains("final"))
            {
                continue;
            }
            if let Some(initializer) = named_children(field).into_iter().find_map(|node| {
                (node.kind() == "variable_declarator"
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|node| text_of(source, node) == name))
                .then(|| node.child_by_field_name("value"))
                .flatten()
            }) {
                return resolve(project, owner, text_of(source, initializer), visiting);
            }
        }
    }
    if qualifier.is_none() {
        let imports = project
            .source(&context.file_path)?
            .lines()
            .filter_map(|line| line.trim().strip_prefix("import static "))
            .map(|line| line.trim_end_matches(';').trim())
            .filter(|import| import.rsplit('.').next() == Some(name))
            .collect::<Vec<_>>();
        if let [import] = imports.as_slice() {
            return resolve(project, context, import, visiting);
        }
    }
    Ok(None)
}
