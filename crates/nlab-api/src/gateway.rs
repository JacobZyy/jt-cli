use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use tree_sitter::{Node, Parser};

use crate::java::parse_java_type;
use crate::model::TypeRef;

pub(crate) const CONTEXT_PACKAGE: &str = "com.zhuanzhuan.arch.zgateway.support";

pub(crate) fn method_key(owner: &str, name: &str, parameters: &[TypeRef]) -> String {
    format!(
        "{owner}#{name}({})",
        parameters
            .iter()
            .map(TypeRef::render_java)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Read declarations, not imports/comments mentioning a convention. No SDK source is needed
/// for an explicit import or a fully qualified context type.
pub(crate) fn methods(source: &str) -> Result<BTreeSet<String>> {
    if !source.contains("ServiceContract") || !source.contains(CONTEXT_PACKAGE) {
        return Ok(BTreeSet::new());
    }
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("parse gateway declarations")?;
    if tree.root_node().has_error() {
        bail!("invalid Java source while identifying gateway methods");
    }
    let declarations = children(tree.root_node());
    let imports = declarations
        .iter()
        .filter(|node| node.kind() == "import_declaration")
        .map(|node| {
            text(source, *node)
                .trim_start_matches("import")
                .trim()
                .trim_end_matches(';')
                .trim()
        })
        .filter(|name| !name.starts_with("static "))
        .collect::<Vec<_>>();
    let package = declarations
        .iter()
        .find(|node| node.kind() == "package_declaration")
        .map(|node| {
            text(source, *node)
                .trim_start_matches("package")
                .trim()
                .trim_end_matches(';')
                .trim()
        });
    let mut result = BTreeSet::new();
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
        let owner = text(
            source,
            interface
                .child_by_field_name("name")
                .context("interface name missing")?,
        );
        let body = interface
            .child_by_field_name("body")
            .context("interface body missing")?;
        for method in children(body)
            .into_iter()
            .filter(|node| node.kind() == "method_declaration")
        {
            let name = text(
                source,
                method
                    .child_by_field_name("name")
                    .context("method name missing")?,
            );
            let parameters = method
                .child_by_field_name("parameters")
                .context("method parameters missing")?;
            let parameters = parameter_types(source, parameters)
                .with_context(|| format!("parse gateway parameters: {owner}#{name}"))?;
            let Some(first) = parameters.first() else {
                continue;
            };
            let response = method
                .child_by_field_name("type")
                .and_then(|node| parse_java_type(text(source, node)))
                .with_context(|| format!("parse gateway return type: {owner}#{name}"))?;
            if has_legacy_type(&response) || parameters.iter().any(has_legacy_type) {
                continue;
            }
            let imported = imports
                .iter()
                .find(|import| import.rsplit('.').next() == Some(first.name.as_str()));
            let qualified = imported.copied().unwrap_or(&first.name);
            let context = if qualified.contains('.') {
                qualified
                    .rsplit_once('.')
                    .is_some_and(|(package, _)| package == CONTEXT_PACKAGE)
            } else if package == Some(CONTEXT_PACKAGE) {
                true
            } else {
                if imports.contains(&format!("{CONTEXT_PACKAGE}.*").as_str()) {
                    bail!(
                        "cannot resolve first parameter {} of {owner}#{name} from wildcard imports; use an explicit context type import",
                        first.name
                    );
                }
                false
            };
            if context && first.array_depth == 0 {
                result.insert(method_key(owner, name, &parameters));
            }
        }
    }
    Ok(result)
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

fn has_legacy_type(value: &TypeRef) -> bool {
    value.simple_name().starts_with("ZZOpen") || value.arguments.iter().any(has_legacy_type)
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
    fn identifies_each_method_by_first_parameter_package_not_names() {
        let source = r#"
package business;
import com.zhuanzhuan.arch.zgateway.support.AnyContext;
import another.EmployeeUser;
@ServiceContract interface OrdersRemote {
    ApiResult<String> selected(@Valid final AnyContext context, String keyword);
    String empty(AnyContext context);
    String qualified(com.zhuanzhuan.arch.zgateway.support.OtherContext context);
    String internal(String keyword);
    String wrongOrder(String keyword, AnyContext context);
    String sameName(EmployeeUser context);
    String wrongPackage(com.zhuanzhuan.arch.zgateway.supported.AnyContext context);
    ZZOpenScfBaseResult<String> legacy(AnyContext context);
    String legacyRequest(AnyContext context, List<ZZOpenRequest> request);
    String overloaded(AnyContext context);
    String overloaded(String keyword);
}
interface Internal { String ignored(AnyContext context); }
// @ServiceContract interface Comment { String ignored(AnyContext context); }
"#;
        assert_eq!(
            methods(source).unwrap(),
            BTreeSet::from([
                "OrdersRemote#selected(AnyContext,String)".to_owned(),
                "OrdersRemote#empty(AnyContext)".to_owned(),
                "OrdersRemote#qualified(com.zhuanzhuan.arch.zgateway.support.OtherContext)"
                    .to_owned(),
                "OrdersRemote#overloaded(AnyContext)".to_owned(),
            ])
        );
        assert!(methods("import com.zhuanzhuan.arch.zgateway.support.*; @ServiceContract interface Remote { String read(EmployeeUser user); }")
            .unwrap_err().to_string().contains("explicit context type import"));
    }
}
