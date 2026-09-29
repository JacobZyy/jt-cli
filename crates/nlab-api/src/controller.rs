use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use tree_sitter::{Node, Parser, Tree};

use crate::graph::{GraphNode, Snapshot};
use crate::java::parse_method_signature;
use crate::model::{InputLocation, RouteSource, TypeRef};
use crate::routes::{BindingSource, HttpRouteKey, RequestBinding};

pub(crate) fn has_controller(source: &str) -> Result<bool> {
    if !source.contains("Controller") {
        return Ok(false);
    }
    let tree = parse(source)?;
    Ok(classes(tree.root_node()).into_iter().any(|class| {
        annotations(class).into_iter().any(|annotation| {
            matches!(
                spring_annotation(source, annotation),
                Some("RestController" | "Controller")
            )
        })
    }))
}

pub(crate) fn routes(
    root: &Path,
    graph: &Snapshot,
    candidates: &[(&GraphNode, &GraphNode)],
) -> Result<Vec<HttpRouteKey>> {
    let files = candidates
        .iter()
        .filter(|(owner, _)| owner.kind == "class")
        .map(|(owner, _)| owner.file_path.as_str())
        .collect::<BTreeSet<_>>();
    let mut routes = Vec::new();
    for file in files {
        let source = fs::read_to_string(graph.source_path(root, file))?;
        if !source.contains("Controller") {
            continue;
        }
        let tree = parse(&source).with_context(|| format!("read Controller {file}"))?;
        for class in classes(tree.root_node()) {
            let annotations = annotations(class);
            let rest = annotations.iter().any(|annotation| {
                spring_annotation(&source, *annotation) == Some("RestController")
            });
            if !rest
                && !annotations
                    .iter()
                    .any(|annotation| spring_annotation(&source, *annotation) == Some("Controller"))
            {
                continue;
            }
            let name = text(
                &source,
                class
                    .child_by_field_name("name")
                    .context("Controller name missing")?,
            );
            let Some(owner) = candidates
                .iter()
                .map(|(owner, _)| *owner)
                .find(|owner| owner.file_path == file && owner.name == name)
            else {
                continue;
            };
            let base = mapping(&source, class)?.unwrap_or_default();
            let body = class
                .child_by_field_name("body")
                .context("Controller body missing")?;
            for method in children(body)
                .into_iter()
                .filter(|node| node.kind() == "method_declaration")
            {
                let Some(mapping) = mapping(&source, method).with_context(|| {
                    format!(
                        "Controller mapping at {file}:{}",
                        method.start_position().row + 1
                    )
                })?
                else {
                    continue;
                };
                let method_name = text(
                    &source,
                    method
                        .child_by_field_name("name")
                        .context("Controller method name missing")?,
                );
                let result = (|| {
                    if !rest
                        && !annotations
                            .iter()
                            .copied()
                            .chain(self::annotations(method))
                            .any(|annotation| {
                                spring_annotation(&source, annotation) == Some("ResponseBody")
                            })
                    {
                        bail!("view Controllers without @ResponseBody are unsupported");
                    }
                    let parameters = method
                        .child_by_field_name("parameters")
                        .context("Controller parameters missing")?;
                    let types = crate::gateway::parameter_types(&source, parameters)
                        .context("unsupported Controller parameter type")?;
                    let methods = candidates
                        .iter()
                        .filter(|(candidate, method)| {
                            candidate.id == owner.id
                                && method.name == method_name
                                && parse_method_signature(&method.signature)
                                    .is_some_and(|(_, parameters)| parameters == types)
                        })
                        .map(|(_, method)| *method)
                        .collect::<Vec<_>>();
                    let [_] = methods.as_slice() else {
                        bail!("Controller method does not match one indexed declaration");
                    };
                    let bindings = request_bindings(&source, parameters)?;
                    let has_body = bindings.iter().any(|binding| {
                        matches!(
                            binding.source,
                            BindingSource::Input(InputLocation::Body | InputLocation::Form, _)
                        )
                    });
                    let verb = mapping
                        .method
                        .or_else(|| base.method.clone())
                        .unwrap_or_else(|| if has_body { "POST" } else { "GET" }.to_owned());
                    if has_body && matches!(verb.as_str(), "GET" | "HEAD") {
                        bail!("{verb} with a request body is unsupported");
                    }
                    let path = match (
                        base.path.trim_matches('/'),
                        mapping.path.trim_start_matches('/'),
                    ) {
                        ("", path) | (path, "") => format!("/{path}"),
                        (base, path) => format!("/{base}/{path}"),
                    };
                    Ok(HttpRouteKey {
                        interface_name: owner.qualified_name.replace("::", "."),
                        method_name: method_name.to_owned(),
                        signature: Some(format!(
                            "{method_name}({})",
                            types
                                .iter()
                                .map(|kind| kind.render_java())
                                .collect::<Vec<_>>()
                                .join(",")
                        )),
                        method: verb,
                        path,
                        host: None,
                        source: RouteSource::Controller,
                        request_bindings: Some(bindings),
                    })
                })()
                .with_context(|| {
                    format!(
                        "Controller {}#{method_name} at {file}:{}",
                        owner.qualified_name,
                        method.start_position().row + 1
                    )
                })?;
                routes.push(result);
            }
        }
    }
    routes.sort_by(|left, right| {
        (&left.interface_name, &left.method_name, &left.signature).cmp(&(
            &right.interface_name,
            &right.method_name,
            &right.signature,
        ))
    });
    Ok(routes)
}

#[derive(Default)]
struct Mapping {
    path: String,
    method: Option<String>,
}

fn mapping(source: &str, declaration: Node<'_>) -> Result<Option<Mapping>> {
    let annotations = annotations(declaration)
        .into_iter()
        .filter(|annotation| {
            matches!(
                spring_annotation(source, *annotation),
                Some(
                    "RequestMapping"
                        | "GetMapping"
                        | "PostMapping"
                        | "PutMapping"
                        | "PatchMapping"
                        | "DeleteMapping"
                )
            )
        })
        .collect::<Vec<_>>();
    let annotation = match annotations.as_slice() {
        [] => return Ok(None),
        [annotation] => *annotation,
        _ => bail!("multiple Controller mapping annotations are unsupported"),
    };
    for name in ["params", "headers", "consumes", "produces"] {
        if attribute(source, annotation, name).is_some() {
            bail!("Controller mapping condition {name} is unsupported");
        }
    }
    let path = attribute(source, annotation, "path")
        .or_else(|| attribute(source, annotation, "value"))
        .map(|value| string_value(source, value))
        .transpose()?
        .unwrap_or_default();
    if path.contains(['{', '}', '*', '$', '#', '?']) {
        bail!("dynamic Controller path is unsupported: {path}");
    }
    let method = match spring_annotation(source, annotation) {
        Some("GetMapping") => Some("GET".to_owned()),
        Some("PostMapping") => Some("POST".to_owned()),
        Some("PutMapping") => Some("PUT".to_owned()),
        Some("PatchMapping") => Some("PATCH".to_owned()),
        Some("DeleteMapping") => Some("DELETE".to_owned()),
        _ => attribute(source, annotation, "method")
            .map(|node| {
                let node = single_value(node)?;
                let value = text(source, node).rsplit('.').next().unwrap_or_default();
                if !matches!(
                    value,
                    "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
                ) {
                    bail!("unsupported Controller HTTP method: {value}");
                }
                Ok(value.to_owned())
            })
            .transpose()?,
    };
    Ok(Some(Mapping { path, method }))
}

fn request_bindings(source: &str, parameters: Node<'_>) -> Result<Vec<RequestBinding>> {
    let types = crate::gateway::parameter_types(source, parameters)
        .context("unsupported Controller parameters")?;
    let parameters = children(parameters)
        .into_iter()
        .filter(|node| !matches!(node.kind(), "line_comment" | "block_comment"))
        .collect::<Vec<_>>();
    let multipart = types.iter().any(|kind| {
        kind.simple_name() == "MultipartFile"
            || kind
                .arguments
                .iter()
                .any(|kind| kind.simple_name() == "MultipartFile")
    });
    parameters
        .into_iter()
        .zip(types)
        .enumerate()
        .map(|(index, (parameter, kind))| {
            let java_name = text(
                source,
                parameter
                    .child_by_field_name("name")
                    .context("Controller parameter name missing")?,
            );
            let annotations = annotations(parameter);
            let bindings = annotations
                .into_iter()
                .filter_map(|annotation| {
                    spring_annotation(source, annotation).map(|name| (name, annotation))
                })
                .collect::<Vec<_>>();
            let source = match bindings.as_slice() {
                [("RequestBody", _)] if !multipart => {
                    BindingSource::Input(InputLocation::Body, None)
                }
                [("RequestParam", annotation)] => {
                    let name = attribute(source, *annotation, "name")
                        .or_else(|| attribute(source, *annotation, "value"))
                        .map(|value| string_value(source, value))
                        .transpose()?;
                    let name = if name.is_none()
                        && matches!(kind.simple_name(), "Map" | "MultiValueMap")
                    {
                        None
                    } else {
                        Some(name.unwrap_or_else(|| java_name.to_owned()))
                    };
                    BindingSource::Input(
                        if multipart {
                            InputLocation::Form
                        } else {
                            InputLocation::Query
                        },
                        name,
                    )
                }
                [] | [("ModelAttribute", _)] => {
                    if matches!(
                        kind.simple_name(),
                        "HttpServletRequest"
                            | "HttpServletResponse"
                            | "HttpSession"
                            | "BindingResult"
                    ) {
                        BindingSource::Context
                    } else {
                        BindingSource::Input(
                            if multipart {
                                InputLocation::Form
                            } else {
                                InputLocation::Query
                            },
                            is_scalar(&kind).then(|| java_name.to_owned()),
                        )
                    }
                }
                _ => bail!(
                    "unsupported Controller binding on parameter {java_name}: {}",
                    bindings
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            if matches!(source, BindingSource::Input(InputLocation::Form, None)) {
                bail!("multipart object binding requires explicit fields: {java_name}");
            }
            if matches!(source, BindingSource::Input(InputLocation::Form, _)) {
                let collection = matches!(kind.simple_name(), "List" | "Set" | "Collection");
                let value = if collection {
                    kind.arguments.first().unwrap_or(&kind)
                } else {
                    &kind
                };
                if !is_scalar(value)
                    || kind.array_depth > 1
                    || (collection && (kind.array_depth > 0 || value.array_depth > 0))
                {
                    bail!("multipart field must be a scalar or file, or a collection of them: {java_name}");
                }
            }
            Ok(RequestBinding { index, source })
        })
        .collect()
}

fn is_scalar(kind: &TypeRef) -> bool {
    matches!(
        kind.simple_name(),
        "String"
            | "char"
            | "Character"
            | "boolean"
            | "Boolean"
            | "byte"
            | "Byte"
            | "short"
            | "Short"
            | "int"
            | "Integer"
            | "long"
            | "Long"
            | "float"
            | "Float"
            | "double"
            | "Double"
            | "BigDecimal"
            | "BigInteger"
            | "Date"
            | "LocalDate"
            | "LocalDateTime"
            | "MultipartFile"
    )
}

fn parse(source: &str) -> Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("parse Controller source")?;
    if tree.root_node().has_error() {
        bail!("invalid Controller Java source");
    }
    Ok(tree)
}

fn classes(node: Node<'_>) -> Vec<Node<'_>> {
    children(node)
        .into_iter()
        .filter(|node| node.kind() == "class_declaration")
        .collect()
}

fn annotations(node: Node<'_>) -> Vec<Node<'_>> {
    children(node)
        .into_iter()
        .filter(|node| node.kind() == "modifiers")
        .flat_map(children)
        .filter(|node| matches!(node.kind(), "annotation" | "marker_annotation"))
        .collect()
}

fn spring_annotation<'a>(source: &'a str, annotation: Node<'_>) -> Option<&'a str> {
    let name = text(source, annotation.child_by_field_name("name")?);
    if let Some(name) = name.strip_prefix("org.springframework.web.bind.annotation.") {
        return Some(name);
    }
    if name == "org.springframework.stereotype.Controller" {
        return Some("Controller");
    }
    let package = if name == "Controller" {
        "org.springframework.stereotype"
    } else {
        "org.springframework.web.bind.annotation"
    };
    let mut root = annotation;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    let mut wildcard = false;
    for import in children(root)
        .into_iter()
        .filter(|node| node.kind() == "import_declaration")
    {
        let parts = children(import);
        let Some(imported) = parts
            .iter()
            .find(|node| matches!(node.kind(), "identifier" | "scoped_identifier"))
            .map(|node| text(source, *node))
        else {
            continue;
        };
        if imported.rsplit('.').next() == Some(name) {
            return (imported == format!("{package}.{name}")).then_some(name);
        }
        wildcard |= imported == package && parts.iter().any(|node| node.kind() == "asterisk");
    }
    wildcard.then_some(name)
}

fn attribute<'a>(source: &str, annotation: Node<'a>, name: &str) -> Option<Node<'a>> {
    let arguments = annotation.child_by_field_name("arguments")?;
    children(arguments).into_iter().find_map(|node| {
        if node.kind() == "element_value_pair" {
            return (text(source, node.child_by_field_name("key")?) == name)
                .then(|| node.child_by_field_name("value"))
                .flatten();
        }
        (name == "value").then_some(node)
    })
}

fn single_value(node: Node<'_>) -> Result<Node<'_>> {
    if node.kind() != "element_value_array_initializer" {
        return Ok(node);
    }
    let values = children(node);
    match values.as_slice() {
        [value] => Ok(*value),
        _ => bail!("multiple Controller paths or HTTP methods are unsupported"),
    }
}

fn string_value(source: &str, node: Node<'_>) -> Result<String> {
    let node = single_value(node)?;
    if node.kind() != "string_literal" {
        bail!(
            "Controller mapping must use a literal string: {}",
            text(source, node)
        );
    }
    serde_json::from_str(text(source, node)).context("decode Controller mapping string")
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
    use crate::graph::{GraphEdge, test_snapshot};
    use crate::java::JavaProject;
    use crate::model::{ContractIr, TargetIdentity};

    #[test]
    fn controller_routes_feed_shared_contract_and_generators() {
        let source = r#"package p;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.multipart.MultipartFile;
@RestController
@RequestMapping("/orders")
class OrdersController {
    @GetMapping
    Result<String> delete(@RequestParam("q") String keyword) { return null; }
    @PostMapping
    Result<String> save(@RequestBody Payload payload) { return null; }
    @PostMapping("/mixed")
    Result<String> mixed(@RequestBody Payload payload, @RequestParam("q") String keyword) { return null; }
    @PostMapping("/upload")
    Result<String> upload(@RequestParam("file") MultipartFile file, @RequestParam("count") Integer count) { return null; }
    @RequestMapping("/any")
    Result<String> export(@RequestBody Payload payload) { return null; }
    @GetMapping("/byId")
    Result<String> find(Long id) { return null; }
    @GetMapping("/byCode")
    Result<String> find(String code) { return null; }
    String internal() { return null; }
    // @PostMapping("/deleted") String deleted() { return null; }
}
class Payload { String value; }
"#;
        let repo = tempfile::tempdir().unwrap();
        fs::create_dir(repo.path().join("web")).unwrap();
        let path = repo.path().join("web/OrdersController.java");
        fs::write(&path, source).unwrap();
        let node = |name: &str, kind: &str, signature: &str| GraphNode {
            id: name.to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            qualified_name: format!("p::{name}"),
            file_path: "web/OrdersController.java".to_owned(),
            start_line: 1,
            start_column: 0,
            docstring: None,
            signature: signature.to_owned(),
            decorators: String::new(),
            return_type: String::new(),
        };
        let mut nodes = vec![
            node("OrdersController", "class", ""),
            node("Payload", "class", ""),
            node("value", "field", "String value"),
        ];
        let methods = [
            ("delete", "String keyword"),
            ("save", "Payload payload"),
            ("mixed", "Payload payload, String keyword"),
            ("upload", "MultipartFile file, Integer count"),
            ("export", "Payload payload"),
            ("internal", ""),
        ];
        nodes.extend(methods.iter().map(|(name, parameters)| {
            node(name, "method", &format!("Result<String> ({parameters})"))
        }));
        for (id, parameters) in [("findById", "Long id"), ("findByCode", "String code")] {
            let mut method = node(id, "method", &format!("Result<String> ({parameters})"));
            method.name = "find".to_owned();
            nodes.push(method);
        }
        let edges = methods
            .iter()
            .map(|(name, _)| ("OrdersController", *name))
            .chain([("Payload", "value")])
            .chain([
                ("OrdersController", "findById"),
                ("OrdersController", "findByCode"),
            ])
            .map(|(source, target)| GraphEdge {
                source: source.to_owned(),
                target: target.to_owned(),
                kind: "contains".to_owned(),
                line: 0,
                column: 0,
                metadata: String::new(),
                provenance: String::new(),
            })
            .collect();
        let graph = test_snapshot(nodes, edges);
        let candidates = graph
            .contained("OrdersController", "method")
            .into_iter()
            .map(|method| (&graph.nodes["OrdersController"], method))
            .collect::<Vec<_>>();
        assert!(has_controller(source).unwrap());
        assert!(!has_controller("// @RestController\nclass Internal {}").unwrap());
        assert!(has_controller("import org.springframework.web.bind.annotation.RestController; @RestController class Web {}").unwrap());
        assert!(!has_controller("import org.springframework.web.bind.annotation.*; import local.RestController; @RestController class Internal {}").unwrap());
        let routes = routes(repo.path(), &graph, &candidates).unwrap();
        assert_eq!(routes.len(), 7);
        assert_eq!(
            routes
                .iter()
                .find(|route| route.method_name == "export")
                .unwrap()
                .method,
            "POST"
        );
        let project = JavaProject::load(repo.path(), &graph).unwrap();
        let (operations, schemas) = project
            .build_contracts(&["web".to_owned()], &routes)
            .unwrap();
        assert_eq!(operations.len(), 7);
        assert!(operations.iter().all(|operation| operation.route.source
            == RouteSource::Controller
            && operation.response.name == "String"));
        let ir = ContractIr {
            operations,
            schemas,
            target: TargetIdentity {
                app_name: "test".to_owned(),
                branch: "main".to_owned(),
                commit: "commit".to_owned(),
                codegraph_version: "test".to_owned(),
                codegraph_extraction_version: "test".to_owned(),
            },
        };
        let config = serde_json::from_value(serde_json::json!({
            "version": 2, "backend": {"repository": "git@example.com:team/backend.git", "branch": "main", "appName": "test", "contractRoots": ["web"]},
            "frontend": {
                "sourceRoot": "src", "buildTool": {"kind": "other", "testConfigs": []}, "tsconfigPath": "tsconfig.json",
                "request": {"module": "@/request", "export": "nlabRequest", "responseMode": "unwrapped"},
                "response": {"successCode": "0", "codeFields": ["code"], "dataFields": ["data"], "mockCodeField": "code", "mockDataField": "data"},
                "layout": {"preset": "service", "implementationDir": "src/service", "typesDir": "src/types", "enumsDir": "src/enums"},
                "aliases": {"enabled": false, "implementation": "@service", "types": "@types", "enums": "@enums"}
            }
        })).unwrap();
        let openapi = crate::openapi::generate(&ir, &config).unwrap();
        let document: serde_json::Value = serde_json::from_str(&openapi.source).unwrap();
        assert_eq!(
            document["paths"]["/orders"]["get"]["parameters"][0]["name"],
            "q"
        );
        assert!(document["paths"]["/orders"]["post"]["requestBody"].is_object());
        assert_eq!(
            document["paths"]["/orders/upload"]["post"]["requestBody"]["content"]["multipart/form-data"]
                ["schema"]["properties"]["file"]["format"],
            "binary"
        );
        let frontend = crate::typescript::generate(&ir, &config).unwrap();
        let api = &frontend.files[&frontend.api_files[0]];
        assert!(api.contains("file?: Blob"));
        assert!(api.contains("function findByLong("));
        assert!(api.contains("function findByString("));
        assert!(api.contains("function deleteApi("));
        assert!(api.contains("function exportApi("));
        assert!(api.contains("const form = new FormData()"));
        assert!(api.contains("data: form"));
        assert!(api.contains("data: request?.[\"payload\"]"));
        assert!(api.contains("params: { q: request?.[\"q\"] }"));
        fs::write(&path, source.replace("/mixed", "/{id}")).unwrap();
        assert!(
            format!(
                "{:#}",
                super::routes(repo.path(), &graph, &candidates).unwrap_err()
            )
            .contains("dynamic Controller path")
        );
    }
}
