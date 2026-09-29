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
    values_source(source, node, enum_name, 0)
}

fn values_source(source: &str, node: Node<'_>, enum_name: &str, depth: usize) -> bool {
    if depth >= 8 {
        return false;
    }
    if node.kind() == "identifier" {
        let name = text_of(source, node);
        let Some(scope) = ancestors(node).find(|node| node.kind() == "method_declaration") else {
            return false;
        };
        let nodes = descendants(scope);
        if nodes.iter().any(|node| {
            matches!(node.kind(), "assignment_expression" | "update_expression")
                && text_of(source, *node).starts_with(name)
        }) {
            return false;
        }
        let initializers = nodes
            .into_iter()
            .filter(|variable| {
                variable.kind() == "variable_declarator"
                    && variable.start_byte() < node.start_byte()
                    && variable
                        .child_by_field_name("name")
                        .is_some_and(|variable| text_of(source, variable) == name)
            })
            .filter_map(|variable| variable.child_by_field_name("value"))
            .collect::<Vec<_>>();
        return initializers.len() == 1
            && values_source(source, initializers[0], enum_name, depth + 1);
    }
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
            Some(name.to_owned())
        }
        _ => None,
    }
}

fn canonical_projection(source: &str, method: Node<'_>, accessor: String) -> String {
    if getter_signal(&accessor).is_some() || accessor == "name" {
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

pub(super) fn lookup_projection(
    source: &str,
    method: Node<'_>,
    enum_name: &str,
) -> Option<(String, bool)> {
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
    if writes.is_empty() {
        let initializer = nodes.iter().find_map(|node| {
            (node.kind() == "variable_declarator"
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name| text_of(source, name) == map))
            .then(|| node.child_by_field_name("value"))
            .flatten()
        });
        if let Some(initializer) = initializer {
            return collected_map_projection(source, initializer, enum_name);
        }
        // A local map is filled once, then wrapped without exposing the mutable alias.
        let assignments = nodes
            .iter()
            .filter(|node| {
                node.kind() == "assignment_expression"
                    && node
                        .child_by_field_name("left")
                        .is_some_and(|left| text_of(source, left) == map)
            })
            .copied()
            .collect::<Vec<_>>();
        let [assignment] = assignments.as_slice() else {
            return None;
        };
        let value = assignment.child_by_field_name("right")?;
        if invocation_name(source, value) != "unmodifiableMap"
            || !value.child_by_field_name("object").is_some_and(|object| {
                matches!(
                    text_of(source, object),
                    "Collections" | "java.util.Collections"
                )
            })
        {
            return None;
        }
        let args = arguments(value);
        let [local] = args.as_slice() else {
            return None;
        };
        let local = text_of(source, *local);
        let block = ancestors(*assignment).find(|node| node.kind() == "static_initializer")?;
        let local_nodes = descendants(block);
        let calls = local_nodes
            .iter()
            .filter(|node| {
                node.kind() == "method_invocation"
                    && node
                        .child_by_field_name("object")
                        .is_some_and(|object| text_of(source, object) == local)
            })
            .copied()
            .collect::<Vec<_>>();
        let [put] = calls.as_slice() else {
            return None;
        };
        if invocation_name(source, *put) != "put" {
            return None;
        }
        let loop_node = ancestors(*put).find(|node| node.kind() == "enhanced_for_statement")?;
        if !values_call(source, loop_node.child_by_field_name("value")?, enum_name) {
            return None;
        }
        let item = text_of(source, loop_node.child_by_field_name("name")?);
        let args = arguments(*put);
        if args.len() != 2 || text_of(source, args[1]) != item {
            return None;
        }
        let statements = statements(loop_node.child_by_field_name("body")?);
        if statements.len() != 1 || statements[0].kind() != "expression_statement" {
            return None;
        }
        let uses = local_nodes
            .iter()
            .filter(|node| node.kind() == "identifier" && text_of(source, **node) == local)
            .count();
        // Declaration, put receiver, wrapper argument; any other use could leak or mutate it.
        if uses != 3 {
            return None;
        }
        return projection(source, args[0], item);
    }
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

fn collected_map_projection(
    source: &str,
    initializer: Node<'_>,
    enum_name: &str,
) -> Option<String> {
    if invocation_name(source, initializer) != "collect"
        || !values_stream(
            source,
            initializer.child_by_field_name("object")?,
            enum_name,
        )
    {
        return None;
    }
    let args = arguments(initializer);
    let [collector] = args.as_slice() else {
        return None;
    };
    if invocation_name(source, *collector) != "toMap"
        || !collector
            .child_by_field_name("object")
            .is_some_and(|object| {
                matches!(
                    text_of(source, object),
                    "Collectors" | "java.util.stream.Collectors"
                )
            })
    {
        return None;
    }
    let args = arguments(*collector);
    let [key, value] = args.as_slice() else {
        return None;
    };
    let identity = if value.kind() == "lambda_expression" {
        let parameter =
            text_of(source, value.child_by_field_name("parameters")?).trim_matches(['(', ')']);
        text_of(source, value.child_by_field_name("body")?) == parameter
    } else {
        invocation_name(source, *value) == "identity"
            && arguments(*value).is_empty()
            && value.child_by_field_name("object").is_some_and(|object| {
                matches!(
                    text_of(source, object),
                    "Function" | "java.util.function.Function"
                )
            })
    };
    if !identity {
        return None;
    }
    if key.kind() == "method_reference" {
        let parts = named_children(*key);
        if parts.len() == 2 && text_of(source, parts[0]) == enum_name {
            return Some(text_of(source, parts[1]).to_owned());
        }
    } else if key.kind() == "lambda_expression" {
        let parameter =
            text_of(source, key.child_by_field_name("parameters")?).trim_matches(['(', ')']);
        return projection(source, key.child_by_field_name("body")?, parameter);
    }
    None
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
