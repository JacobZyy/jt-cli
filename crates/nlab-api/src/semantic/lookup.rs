use super::*;

#[derive(Clone, Debug)]
pub(super) struct EnumLookup {
    pub domain: Domain,
    pub rejects_unknown: bool,
}

impl SemanticAnalyzer<'_> {
    pub(super) fn index_enum_lookups(&mut self) -> Result<()> {
        let enums = self
            .project
            .graph()
            .nodes
            .values()
            .filter(|node| node.kind == "enum")
            .cloned()
            .collect::<Vec<_>>();
        for enum_node in enums {
            let methods = self
                .project
                .graph()
                .contained(&enum_node.id, "method")
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            for method in methods {
                if method_parameters(&method.signature).len() != 1 {
                    continue;
                }
                let parsed = self.parsed(&method.file_path)?;
                let Some(declaration) = method_declaration(parsed, &method) else {
                    continue;
                };
                let return_type = declaration
                    .child_by_field_name("type")
                    .map(|node| text_of(&parsed.source, node));
                if return_type != Some(enum_node.name.as_str())
                    || !named_children(declaration).iter().any(|node| {
                        node.kind() == "modifiers"
                            && text_of(&parsed.source, *node)
                                .split_whitespace()
                                .any(|word| word == "static")
                    })
                {
                    continue;
                }
                let projection = lookup_projection(&parsed.source, declaration, &enum_node.name)
                    .map(|(accessor, rejects)| {
                        (
                            canonical_projection(&parsed.source, declaration, accessor),
                            rejects,
                        )
                    });
                let (domain, rejects_unknown) = if let Some((accessor, rejects)) = projection {
                    (self.enum_domain(&enum_node, &accessor)?, rejects)
                } else {
                    (
                        incomplete_enum_domain(
                            &enum_node,
                            "",
                            "enum lookup projection is not proven",
                        ),
                        false,
                    )
                };
                self.enum_lookups.insert(
                    method.id.clone(),
                    EnumLookup {
                        domain,
                        rejects_unknown,
                    },
                );
            }
        }
        Ok(())
    }
}

pub(super) fn method_declaration<'a>(
    parsed: &'a ParsedFile,
    method: &GraphNode,
) -> Option<Node<'a>> {
    descendants(parsed.tree.root_node())
        .into_iter()
        .find(|node| {
            matches!(
                node.kind(),
                "method_declaration" | "constructor_declaration"
            ) && node.start_position().row + 1 == method.start_line
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name| text_of(&parsed.source, name) == method.name)
        })
}

fn invocation_name<'a>(source: &'a str, node: Node<'_>) -> &'a str {
    node.child_by_field_name("name")
        .map(|name| text_of(source, name))
        .unwrap_or("")
}

fn arguments(node: Node<'_>) -> Vec<Node<'_>> {
    node.child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default()
}

fn values_call(source: &str, node: Node<'_>, enum_name: &str) -> bool {
    node.kind() == "method_invocation"
        && invocation_name(source, node) == "values"
        && arguments(node).is_empty()
        && node
            .child_by_field_name("object")
            .is_none_or(|object| text_of(source, object) == enum_name)
}

fn values_stream(source: &str, node: Node<'_>, enum_name: &str) -> bool {
    if node.kind() != "method_invocation" || invocation_name(source, node) != "stream" {
        return false;
    }
    let args = arguments(node);
    args.len() == 1
        && values_call(source, args[0], enum_name)
        && node
            .child_by_field_name("object")
            .is_some_and(|object| matches!(text_of(source, object), "Arrays" | "java.util.Arrays"))
}

fn projection(source: &str, node: Node<'_>, item: &str) -> Option<String> {
    let object = node.child_by_field_name("object")?;
    if text_of(source, object) != item {
        return None;
    }
    match node.kind() {
        "field_access" => node
            .child_by_field_name("field")
            .map(|field| text_of(source, field).to_owned()),
        "method_invocation" if arguments(node).is_empty() => {
            let name = invocation_name(source, node);
            getter_signal(name).map(|_| name.to_owned())
        }
        _ => None,
    }
}

fn canonical_projection(source: &str, method: Node<'_>, accessor: String) -> String {
    if getter_signal(&accessor).is_some() {
        return accessor;
    }
    let Some(declaration) = ancestors(method).find(|node| node.kind() == "enum_declaration") else {
        return accessor;
    };
    let getter = format!("get{}", uppercase_first(&accessor));
    if let Some(method) = descendants(declaration).into_iter().find(|node| {
        node.kind() == "method_declaration" && invocation_name(source, *node) == getter
    }) {
        let direct = method
            .child_by_field_name("body")
            .and_then(returned_value)
            .is_some_and(|value| text_of(source, value).trim_start_matches("this.") == accessor);
        return if direct { getter } else { accessor };
    }
    if named_children(declaration)
        .iter()
        .any(|node| node.kind() == "modifiers" && text_of(source, *node).contains("@Getter"))
    {
        return getter;
    }
    accessor
}

fn equality_projection(
    source: &str,
    node: Node<'_>,
    item: &str,
    parameter: &str,
) -> Option<String> {
    let node = unwrap_parentheses(node);
    let operands = match node.kind() {
        "binary_expression"
            if node
                .child_by_field_name("operator")
                .is_some_and(|op| text_of(source, op) == "==") =>
        {
            vec![
                node.child_by_field_name("left")?,
                node.child_by_field_name("right")?,
            ]
        }
        "method_invocation" if invocation_name(source, node) == "equals" => {
            let args = arguments(node);
            let object = node.child_by_field_name("object")?;
            if matches!(text_of(source, object), "Objects" | "java.util.Objects") {
                args
            } else if args.len() == 1 {
                vec![object, args[0]]
            } else {
                return None;
            }
        }
        _ => return None,
    };
    if operands.len() != 2 {
        return None;
    }
    for (value, key) in [(operands[0], operands[1]), (operands[1], operands[0])] {
        if text_of(source, key) == parameter {
            if let Some(accessor) = projection(source, value, item) {
                return Some(accessor);
            }
        }
    }
    None
}

fn returned_value(node: Node<'_>) -> Option<Node<'_>> {
    let statement = if node.kind() == "block" {
        let children = statements(node);
        if children.len() != 1 {
            return None;
        }
        children[0]
    } else {
        node
    };
    (statement.kind() == "return_statement")
        .then(|| named_children(statement).first().copied())
        .flatten()
}

fn lookup_projection(source: &str, method: Node<'_>, enum_name: &str) -> Option<(String, bool)> {
    let params = method
        .child_by_field_name("parameters")
        .map(named_children)?;
    if params.len() != 1 {
        return None;
    }
    let parameter = text_of(source, params[0].child_by_field_name("name")?);
    let body = method.child_by_field_name("body")?;
    let nodes = descendants(body);
    // A complete enumeration with one equality predicate is a reverse enum lookup.
    for node in &nodes {
        if node.kind() == "enhanced_for_statement" {
            if !values_call(source, node.child_by_field_name("value")?, enum_name) {
                continue;
            }
            let item = text_of(source, node.child_by_field_name("name")?);
            let statements = statements(node.child_by_field_name("body")?);
            if statements.len() != 1 || statements[0].kind() != "if_statement" {
                continue;
            }
            let condition = statements[0];
            if condition.child_by_field_name("alternative").is_some() {
                continue;
            }
            let accessor = equality_projection(
                source,
                condition.child_by_field_name("condition")?,
                item,
                parameter,
            )?;
            if text_of(
                source,
                returned_value(condition.child_by_field_name("consequence")?)?,
            ) != item
            {
                continue;
            }
            let rejects = nodes
                .iter()
                .filter(|node| node.kind() == "return_statement")
                .all(|node| {
                    returned_value(*node).is_some_and(|value| {
                        matches!(text_of(source, value), "null") || text_of(source, value) == item
                    })
                });
            return Some((accessor, rejects));
        }
        if node.kind() != "method_invocation" {
            continue;
        }
        let name = invocation_name(source, *node);
        if name == "filter" {
            let stream = node.child_by_field_name("object")?;
            if !values_stream(source, stream, enum_name) {
                continue;
            }
            let args = arguments(*node);
            if args.len() != 1 || args[0].kind() != "lambda_expression" {
                continue;
            }
            let item = text_of(source, args[0].child_by_field_name("parameters")?)
                .trim_matches(['(', ')']);
            let accessor = equality_projection(
                source,
                args[0].child_by_field_name("body")?,
                item,
                parameter,
            )?;
            let first = node.parent()?;
            let fallback = first.parent()?;
            if invocation_name(source, first) != "findFirst"
                || invocation_name(source, fallback) != "orElse"
            {
                continue;
            }
            let fallback_args = arguments(fallback);
            if fallback_args.len() != 1 {
                continue;
            }
            let rejects = text_of(source, fallback_args[0]) == "null"
                && nodes
                    .iter()
                    .filter(|node| node.kind() == "return_statement")
                    .all(|node| {
                        returned_value(*node).is_some_and(|value| {
                            value == fallback || text_of(source, value) == "null"
                        })
                    });
            return Some((accessor, rejects));
        }
        if !matches!(name, "get" | "getOrDefault") {
            continue;
        }
        let args = arguments(*node);
        if args.is_empty() || text_of(source, args[0]) != parameter {
            continue;
        }
        let map = text_of(source, node.child_by_field_name("object")?);
        let enum_body = ancestors(method).find(|node| node.kind() == "enum_body")?;
        let accessor = map_projection(source, enum_body, map, enum_name)?;
        // Restrict the return path to the lookup, optionally preceded by a null-input guard.
        let rejects = name == "get"
            && nodes
                .iter()
                .filter(|node| node.kind() == "return_statement")
                .all(|ret| {
                    returned_value(*ret).is_some_and(|value| {
                        value == *node
                            || text_of(source, value) == "null"
                            || (value.kind() == "ternary_expression"
                                && value
                                    .child_by_field_name("consequence")
                                    .is_some_and(|branch| text_of(source, branch) == "null")
                                && value.child_by_field_name("alternative") == Some(*node))
                    })
                });
        return Some((accessor, rejects));
    }
    None
}

fn map_projection(source: &str, body: Node<'_>, map: &str, enum_name: &str) -> Option<String> {
    let nodes = descendants(body);
    let private_map = nodes.iter().any(|node| {
        node.kind() == "field_declaration"
            && named_children(*node).iter().any(|variable| {
                variable.kind() == "variable_declarator"
                    && variable
                        .child_by_field_name("name")
                        .is_some_and(|name| text_of(source, name) == map)
            })
            && ["private", "static", "final"].iter().all(|word| {
                text_of(source, *node)
                    .split_whitespace()
                    .any(|part| part == *word)
            })
    });
    if !private_map {
        return None;
    }
    let writes = nodes
        .iter()
        .filter(|node| {
            node.kind() == "method_invocation"
                && node
                    .child_by_field_name("object")
                    .is_some_and(|object| text_of(source, object) == map)
                && !matches!(
                    invocation_name(source, **node),
                    "get" | "getOrDefault" | "containsKey"
                )
        })
        .copied()
        .collect::<Vec<_>>();
    if writes.len() != 1 || invocation_name(source, writes[0]) != "put" {
        return None;
    }
    let put = writes[0];
    let lambda = ancestors(put).find(|node| node.kind() == "lambda_expression")?;
    let item = text_of(source, lambda.child_by_field_name("parameters")?).trim_matches(['(', ')']);
    let args = arguments(put);
    if args.len() != 2 || text_of(source, args[1]) != item {
        return None;
    }
    let lambda_body = lambda.child_by_field_name("body")?;
    if lambda_body.kind() == "block" {
        let statements = statements(lambda_body);
        if statements.len() != 1 || statements[0].kind() != "expression_statement" {
            return None;
        }
    } else if lambda_body != put {
        return None;
    }
    let for_each = lambda.parent()?.parent()?;
    if invocation_name(source, for_each) != "forEach"
        || !values_stream(source, for_each.child_by_field_name("object")?, enum_name)
        || !ancestors(for_each).any(|node| node.kind() == "static_initializer")
    {
        return None;
    }
    projection(source, args[0], item)
}

pub(super) fn ancestors(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(node.parent(), |node| node.parent())
}

pub(super) fn statements(node: Node<'_>) -> Vec<Node<'_>> {
    named_children(node)
        .into_iter()
        .filter(|node| !matches!(node.kind(), "line_comment" | "block_comment"))
        .collect()
}

pub(super) fn unwrap_parentheses(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_expression" {
        let Some(child) = node.named_child(0) else {
            break;
        };
        node = child;
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(source: &str) -> Option<(String, bool)> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let method = descendants(tree.root_node())
            .into_iter()
            .find(|node| {
                node.kind() == "method_declaration" && invocation_name(source, *node) == "decode"
            })
            .unwrap();
        lookup_projection(source, method, "Kind")
    }

    #[test]
    fn reverse_lookup_uses_comparison_or_map_key_not_method_name() {
        let loop_lookup = "enum Kind { A; static Kind decode(Integer input) { for (Kind item : values()) { if (Objects.equals(item.type, input)) { return item; } } return null; } }";
        assert_eq!(lookup(loop_lookup), Some(("type".to_owned(), true)));
        assert_eq!(
            lookup(&loop_lookup.replace("return null", "return A")),
            Some(("type".to_owned(), false))
        );
        assert_eq!(
            lookup(&loop_lookup.replace("item.type, input", "item.type, input + 1")),
            None
        );

        let stream = "enum Kind { A; static Kind decode(Integer input) { return Arrays.stream(values()).filter(item -> item.type.equals(input)).findFirst().orElse(null); } }";
        assert_eq!(lookup(stream), Some(("type".to_owned(), true)));
        let map = "enum Kind { A; private static final Map<Integer, Kind> INDEX = new HashMap<>(); static { Arrays.stream(Kind.values()).forEach(item -> INDEX.put(item.getType(), item)); } static Kind decode(Integer input) { return input == null ? null : INDEX.get(input); } }";
        assert_eq!(lookup(map), Some(("getType".to_owned(), true)));
        assert_eq!(
            lookup(&map.replace("INDEX.get(input)", "INDEX.getOrDefault(input, A)")),
            Some(("getType".to_owned(), false))
        );
        assert_eq!(
            lookup(&map.replace(
                "INDEX.put(item.getType(), item)",
                "INDEX.put(item.getType() + 1, item)"
            )),
            None
        );
        assert_eq!(
            lookup(&map.replace("private static final", "public static final")),
            None
        );
    }
}
