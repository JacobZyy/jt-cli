use anyhow::{Context, Result, bail};
use tree_sitter::{Node, Parser};

use crate::java::parse_java_type;
use crate::model::TypeRef;

pub(crate) const CONTEXT_PACKAGE: &str = "com.zhuanzhuan.arch.zgateway.support";

/// Probe directories for interface declarations. HTTP eligibility comes from Gateway routes.
pub(crate) fn has_contract_methods(source: &str) -> Result<bool> {
    if !source.contains("ServiceContract") {
        return Ok(false);
    }
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("parse contract declarations")?;
    if tree.root_node().has_error() {
        bail!("invalid Java source while probing contract directories");
    }
    let declarations = children(tree.root_node());
    for interface in declarations
        .iter()
        .filter(|node| node.kind() == "interface_declaration")
    {
        let modifiers = children_of_kind(*interface, "modifiers");
        let service_contract = modifiers.into_iter().flat_map(children).any(|annotation| {
            matches!(annotation.kind(), "annotation" | "marker_annotation")
                && annotation.child_by_field_name("name").is_some_and(|name| {
                    text(source, name).rsplit('.').next() == Some("ServiceContract")
                })
        });
        if !service_contract {
            continue;
        }
        let body = interface
            .child_by_field_name("body")
            .context("interface body missing")?;
        if children(body)
            .into_iter()
            .any(|node| node.kind() == "method_declaration")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn parameter_types(source: &str, parameters: Node<'_>) -> Option<Vec<TypeRef>> {
    children(parameters)
        .into_iter()
        .filter(|node| node.kind() != "line_comment" && node.kind() != "block_comment")
        .map(|parameter| {
            if parameter.kind() == "spread_parameter" {
                let node = children(parameter)
                    .into_iter()
                    .find(|node| !matches!(node.kind(), "modifiers" | "variable_declarator"))?;
                let mut value = parse_java_type(text(source, node))?;
                value.array_depth += 1;
                return Some(value);
            }
            parse_java_type(text(source, parameter.child_by_field_name("type")?))
        })
        .collect()
}

fn children_of_kind<'a>(node: Node<'a>, kind: &str) -> Vec<Node<'a>> {
    children(node)
        .into_iter()
        .filter(|node| node.kind() == kind)
        .collect()
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn text<'a>(source: &'a str, node: Node<'_>) -> &'a str {
    &source[node.byte_range()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_annotated_interfaces_without_a_parameter_heuristic() {
        let source = r#"
package business;
@ServiceContract interface OrdersRemote {
    String selected(String keyword);
}
interface Internal { String ignored(String keyword); }
// @ServiceContract interface Comment { String ignored(String keyword); }
"#;
        assert!(has_contract_methods(source).unwrap());
        assert!(!has_contract_methods("interface Internal { String ignored(); }").unwrap());
    }
}
