use super::*;
use crate::graph::test_snapshot;
use crate::model::{ContractIr, RouteSource, TargetIdentity};
use crate::routes::HttpRouteKey;

const KIND: &str = r#"package p;
public enum Kind {
    A(Numbers.ONE, "First"), B(2, "Second");
    public final int code;
    private final String desc;
    Kind(int code, String desc) { this.code = code; this.desc = desc; }
    public int getCode() { return code; }
    public int val() { return code; }
    public int code() { return code; }
    public String getDesc() { return desc; }
    public String toString() { return "prefix-" + code; }
}
class Numbers {
    static final int BASE = 1;
    static final int ONE = BASE;
}
"#;

const PAYLOAD: &str = r#"package p;
public class Payload {
    private Integer code;
    private String token;
    public Payload() {}
    public Payload(Integer code) { this.code = code; }
    public void setCode(Integer code) { this.code = code; }
    public void setToken(String token) { this.token = token; }
}
"#;

/// Supply an index boundary without depending on a globally installed CodeGraph executable.
/// All value analysis and generated artifacts below use the production pipeline and Java AST.
fn contract(files: &[(&str, &str)], methods: &[&str]) -> ContractIr {
    let root = tempfile::tempdir().unwrap();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (path, source) in files {
        let path = format!("src/main/java/p/{path}");
        tests::write(root.path(), &path, source);
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{path}");
        index_symbols(
            source,
            &path,
            tree.root_node(),
            "p",
            None,
            &mut nodes,
            &mut edges,
        );
    }
    let graph = test_snapshot(nodes.clone(), edges.clone());
    let project = JavaProject::load(root.path(), &graph).unwrap();
    let mut analyzer = SemanticAnalyzer::new(&project);
    for method in nodes.iter().filter(|node| node.kind == "method") {
        for site in analyzer.method_invocations(method).unwrap() {
            for target in analyzer.resolve_invocation(method, &site).unwrap() {
                edges.push(GraphEdge {
                    source: method.id.clone(),
                    target,
                    kind: "calls".into(),
                    line: site.line,
                    column: site.column,
                    metadata: String::new(),
                    provenance: String::new(),
                });
            }
        }
    }
    // Run the same assertions against a fresh real index with
    // NLAB_API_TEST_REAL_CODEGRAPH=1 cargo test -p nlab-api semantic::coverage_tests
    let graph = if std::env::var_os("NLAB_API_TEST_REAL_CODEGRAPH").is_some() {
        for (program, args) in [
            ("git", vec!["init", "-q"]),
            ("codegraph", vec!["init", "-y"]),
        ] {
            let output = std::process::Command::new(program)
                .args(args)
                .current_dir(root.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{program}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Snapshot::load(root.path()).unwrap()
    } else {
        test_snapshot(nodes, edges)
    };
    let project = JavaProject::load(root.path(), &graph).unwrap();
    let routes = methods
        .iter()
        .map(|name| HttpRouteKey {
            interface_name: "p.Facade".into(),
            method_name: (*name).into(),
            signature: None,
            method: "GET".into(),
            path: format!("/{name}"),
            host: None,
            source: RouteSource::Controller,
            request_bindings: Some(
                graph
                    .nodes
                    .values()
                    .find(|node| {
                        node.kind == "method"
                            && node.name == *name
                            && node.qualified_name == format!("p::Facade::{name}")
                    })
                    .map(|method| {
                        method_parameters(&method.signature)
                            .into_iter()
                            .enumerate()
                            .map(|(index, _)| crate::routes::RequestBinding {
                                index,
                                source: crate::routes::BindingSource::Input(
                                    crate::model::InputLocation::Body,
                                    None,
                                ),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
        })
        .collect::<Vec<_>>();
    let (mut operations, mut schemas) = project.build_contracts(&["".into()], &routes).unwrap();
    assert_eq!(operations.len(), methods.len());
    let mut analyzer = SemanticAnalyzer::new(&project);
    analyzer.enrich_linked_enums(&mut schemas).unwrap();
    analyzer.enrich(&mut operations, &schemas).unwrap();
    ContractIr {
        target: TargetIdentity {
            app_name: "coverage".into(),
            branch: "test".into(),
            commit: "test".into(),
            codegraph_version: "fixture".into(),
            codegraph_extraction_version: "fixture".into(),
        },
        operations,
        schemas,
    }
}

fn index_symbols(
    source: &str,
    path: &str,
    node: Node<'_>,
    owner: &str,
    parent: Option<&str>,
    nodes: &mut Vec<GraphNode>,
    edges: &mut Vec<GraphEdge>,
) {
    let kind = match node.kind() {
        "class_declaration" => "class",
        "interface_declaration" => "interface",
        "enum_declaration" => "enum",
        "method_declaration" | "constructor_declaration" => "method",
        "enum_constant" => "constant",
        _ => "",
    };
    let mut next_owner = owner.to_owned();
    let mut next_parent = parent.map(ToOwned::to_owned);
    if !kind.is_empty() {
        let name = text_of(source, node.child_by_field_name("name").unwrap());
        let id = format!("{path}:{}", node.start_byte());
        let fqn = format!("{owner}::{name}");
        let return_type = node
            .child_by_field_name("type")
            .map(|node| text_of(source, node))
            .unwrap_or(name);
        let signature = if kind == "method" {
            format!(
                "{return_type} {}",
                node.child_by_field_name("parameters")
                    .map(|node| text_of(source, node))
                    .unwrap_or("()")
            )
        } else {
            String::new()
        };
        let mut symbol = tests::node(
            &id,
            kind,
            name,
            &fqn,
            path,
            node.start_position().row + 1,
            &signature,
        );
        symbol.return_type = return_type.to_owned();
        symbol.decorators = named_children(node)
            .into_iter()
            .find(|node| node.kind() == "modifiers")
            .map(|node| text_of(source, node).to_owned())
            .unwrap_or_default();
        nodes.push(symbol);
        if let Some(parent) = parent {
            edges.push(tests::contains(parent, &id));
        }
        next_owner = fqn;
        next_parent = Some(id);
    }
    if node.kind() == "field_declaration" {
        for variable in named_children(node)
            .into_iter()
            .filter(|node| node.kind() == "variable_declarator")
        {
            let name = text_of(source, variable.child_by_field_name("name").unwrap());
            let id = format!("{path}:{}", variable.start_byte());
            let signature = format!(
                "{} {name}",
                text_of(source, node.child_by_field_name("type").unwrap())
            );
            let mut field = tests::node(
                &id,
                "field",
                name,
                &format!("{owner}::{name}"),
                path,
                node.start_position().row + 1,
                &signature,
            );
            field.decorators = text_of(source, node).to_owned();
            nodes.push(field);
            if let Some(parent) = parent {
                edges.push(tests::contains(parent, &id));
            }
        }
        return;
    }
    if kind == "method" || kind == "constant" {
        return;
    }
    for child in named_children(node) {
        index_symbols(
            source,
            path,
            child,
            &next_owner,
            next_parent.as_deref(),
            nodes,
            edges,
        );
    }
}

fn patch<'a>(ir: &'a ContractIr, method: &str, field: &str) -> &'a SemanticPatch {
    ir.operations
        .iter()
        .find(|operation| operation.method_name == method)
        .unwrap()
        .semantic_patches
        .iter()
        .find(|patch| {
            patch.target.source == FieldSource::Response && patch.target.field_name == field
        })
        .unwrap()
}

fn values(patch: &SemanticPatch) -> Vec<WireValue> {
    patch
        .associated_values()
        .unwrap_or_else(|| panic!("unassociated: {patch:#?}"))
        .iter()
        .map(|value| value.value.clone())
        .collect()
}

#[test]
fn enum_projections_and_extra_members_reach_generated_types() {
    let source = r#"package p;
import org.apache.commons.lang3.StringUtils;
public class Facade {
    public Payload getter() { Payload v = new Payload(); v.setCode(Kind.A.getCode()); return v; }
    public Payload val() { Payload v = new Payload(); v.setCode(Kind.A.val()); return v; }
    public Payload code() { Payload v = new Payload(); v.setCode(Kind.A.code()); return v; }
    public Payload field() { Payload v = new Payload(); v.setCode(Kind.A.code); return v; }
    public Payload name() { Payload v = new Payload(); v.setToken(Kind.A.name()); return v; }
    public Payload extra() { Payload v = new Payload(); v.setToken(flag() ? Kind.A.name() : "ALL_MARKET"); return v; }
    public Payload empty() { Payload v = new Payload(); v.setToken(flag() ? Kind.A.name() : StringUtils.EMPTY); return v; }
    public Payload label() { Payload v = new Payload(); v.setToken(Kind.A.getDesc()); return v; }
    public Payload transformed() { Payload v = new Payload(); v.setToken(Kind.A.toString()); return v; }
    boolean flag() { return Boolean.getBoolean("flag"); }
}

"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Facade.java", source),
        ],
        &[
            "getter",
            "val",
            "code",
            "field",
            "name",
            "extra",
            "empty",
            "label",
            "transformed",
        ],
    );
    for method in ["getter", "val", "code", "field"] {
        assert_eq!(
            values(patch(&ir, method, "code")),
            vec![WireValue::Number(1), WireValue::Number(2)]
        );
    }
    assert_eq!(
        values(patch(&ir, "name", "token")),
        vec![WireValue::String("A".into()), WireValue::String("B".into())]
    );
    assert!(values(patch(&ir, "extra", "token")).contains(&WireValue::String("ALL_MARKET".into())));
    assert!(values(patch(&ir, "empty", "token")).contains(&WireValue::String(String::new())));
    for method in ["label", "transformed"] {
        assert!(patch(&ir, method, "token").associated_values().is_none());
    }
    assert_ne!(
        crate::openapi::enum_identity(patch(&ir, "name", "token")),
        crate::openapi::enum_identity(patch(&ir, "extra", "token"))
    );
    let mut config = crate::typescript::tests::config();
    config.backend.contract_roots = vec!["src/main/java/p".into()];
    let artifact = crate::typescript::generate(&ir, &config).unwrap();
    let enums = artifact
        .enum_files
        .iter()
        .map(|path| &artifact.files[path])
        .collect::<Vec<_>>();
    assert_eq!(enums.len(), 4, "{enums:#?}");
    assert_eq!(
        enums
            .iter()
            .filter(|source| source.contains("ALL_MARKET"))
            .count(),
        1
    );
    assert!(
        artifact
            .type_files
            .iter()
            .any(|path| artifact.files[path].contains("token: KindName"))
    );
}

#[test]
fn writes_returns_callbacks_and_defaults_reach_enum_members() {
    let payload = PAYLOAD
        .replace("private Integer code;", "public Integer code = 99;")
        .replace(
            "public class Payload",
            "@lombok.Builder public class Payload",
        );
    let source = r#"package p;
import java.util.Optional;
public class Facade {
    public Payload direct() { Payload v = new Payload(); v.code = Kind.A.code; return v; }
    public Payload constructor() { return new Payload(Kind.A.code()); }
    public Payload builder() { return Payload.builder().code(Kind.A.val()).build(); }
    public Payload helper() { Payload v = new Payload(); v.setCode(identity(Kind.A.getCode())); return v; }
    public Payload mapper() { Payload v = new Payload(); v.setCode(convert(500)); return v; }
    public Payload optional() { Payload v = new Payload(); v.setCode(Optional.ofNullable(Kind.A).map(Kind::getCode).orElse(7)); return v; }
    public Payload callback() { Payload v = new Payload(); Optional.ofNullable(Kind.A).map(k -> k.val()).ifPresent(v::setCode); return v; }
    public Payload conditional() { Payload v = new Payload(); if (Boolean.getBoolean("replace")) v.setCode(Kind.A.getCode()); return v; }
    public Payload isolated() { Payload v = new Payload(); Payload scratch = new Payload(); scratch.setCode(dynamic()); v.setCode(Kind.A.getCode()); return v; }
    public Payload callBindings() { Payload v = new Payload(); v.setCode(identity(Kind.A.getCode())); identity(dynamic()); return v; }
    public Payload unknown() { Payload v = new Payload(); v.setCode(identity(dynamic())); return v; }
    public Payload transformedHelper() { Payload v = new Payload(); v.setCode(transform(Kind.A.code)); return v; }
    public Payload rewrittenHelper() { Payload v = new Payload(); v.setCode(rewrite(Kind.A.code)); return v; }
    int identity(int code) { return code; }
    int convert(int code) { if (code == 500) return Kind.A.val(); return Kind.B.code(); }
    int transform(int code) { return code + 100; }
    int rewrite(int code) { code += 100; return code; }
    int dynamic() { return Integer.parseInt(System.getenv("CODE")); }
}
"#;
    let methods = [
        "direct",
        "constructor",
        "builder",
        "helper",
        "mapper",
        "optional",
        "callback",
        "conditional",
        "isolated",
        "callBindings",
        "unknown",
        "transformedHelper",
        "rewrittenHelper",
    ];
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", &payload),
            ("Facade.java", source),
        ],
        &methods,
    );
    for method in &methods[..10] {
        let members = values(patch(&ir, method, "code"));
        assert!(members.contains(&WireValue::Number(1)), "{method}");
        assert!(members.contains(&WireValue::Number(2)), "{method}");
        assert!(
            members.contains(&WireValue::Number(99)),
            "{method}: initializer must survive conditional writes"
        );
    }
    assert!(values(patch(&ir, "optional", "code")).contains(&WireValue::Number(7)));
    for method in ["unknown", "transformedHelper", "rewrittenHelper"] {
        assert!(
            patch(&ir, method, "code").associated_values().is_none(),
            "{method}"
        );
    }
}

#[test]
fn declared_enums_and_collection_elements_preserve_wire_shape() {
    let payload = r#"package p;
import java.util.List;
public class Payload {
    public Kind state;
    public List<Integer> codes;
    public Integer[] array;
    public JsonKind json;
    public DynamicKind dynamic;
    public java.util.Map<String, Kind> states;
    public void setCodes(List<Integer> codes) { this.codes = codes; }
    public void setArray(Integer[] array) { this.array = array; }
}
enum JsonKind { A(3), B(4); @com.fasterxml.jackson.annotation.JsonValue private final int code; JsonKind(int code) { this.code = code; } }
@com.fasterxml.jackson.databind.annotation.JsonSerialize(using=Serializer.class)
enum DynamicKind { A, B; }
"#;
    let source = r#"package p;
import java.util.Arrays;
import java.util.List;
import java.util.stream.Collectors;
public class Facade {
    public Payload query(Payload input) {
        Payload v = new Payload();
        v.setCodes(Arrays.asList(Kind.A, Kind.B).stream().map(Kind::getCode).collect(Collectors.toList()));
        v.setArray(new Integer[] { Kind.A.getCode(), Kind.B.code });
        return v;
    }
}
"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", payload),
            ("Facade.java", source),
        ],
        &["query"],
    );
    assert_eq!(
        values(patch(&ir, "query", "state")),
        vec![WireValue::String("A".into()), WireValue::String("B".into())]
    );
    assert_eq!(
        values(patch(&ir, "query", "json")),
        vec![WireValue::Number(3), WireValue::Number(4)]
    );
    assert!(patch(&ir, "query", "dynamic").associated_values().is_none());
    assert!(
        ir.operations[0]
            .semantic_patches
            .iter()
            .any(|patch| patch.target.source == FieldSource::Request
                && patch.target.field_name == "state"
                && patch.associated_values().is_some())
    );
    assert_eq!(
        values(patch(&ir, "query", "codes")),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );
    assert_eq!(
        values(patch(&ir, "query", "array")),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );
    let mut config = crate::typescript::tests::config();
    config.backend.contract_roots = vec!["src/main/java/p".into()];
    let artifact = crate::typescript::generate(&ir, &config).unwrap();
    assert!(
        artifact
            .type_files
            .iter()
            .any(|path| artifact.files[path].contains("codes: KindCode[]")
                && artifact.files[path].contains("array: KindCode[]"))
    );
    let artifact = crate::openapi::generate(&ir, &config).unwrap();
    let document: serde_json::Value = serde_json::from_str(&artifact.source).unwrap();
    let schemas = document["components"]["schemas"].as_object().unwrap();
    assert!(
        schemas
            .values()
            .any(|schema| schema["properties"]["codes"]["items"]["enum"]
                == serde_json::json!([1, 2]))
    );
    assert!(
        schemas
            .values()
            .all(|schema| schema["properties"]["codes"].get("enum").is_none())
    );
    assert!(schemas.values().any(
        |schema| schema["properties"]["states"]["additionalProperties"]["enum"]
            == serde_json::json!(["A", "B"])
    ));
    assert!(
        schemas
            .values()
            .any(|schema| schema["properties"]["json"]["type"] == "number"
                && schema["properties"]["json"]["enum"] == serde_json::json!([3, 4]))
    );
}

#[test]
fn constructor_builder_and_implicit_defaults_are_not_lost() {
    let builder = r#"package p;
public class Payload {
    public int code;
    public Payload() {}
    public Payload(int code) { this.code = code; }
    public void setCode(int code) { this.code = code; }
    public static Builder builder() { return new Builder(); }
    public static class Builder {
        private int code;
        public Builder code(int code) { this.code = code; return this; }
        public Payload build() { return new Payload(code); }
    }
}
"#;
    let source = r#"package p;
public class Facade {
    public Payload builder() { return Payload.builder().code(Kind.A.getCode()).build(); }
    public Payload conditional() { Payload v = new Payload(); if (System.currentTimeMillis() > 0) v.setCode(Kind.A.val()); return v; }
    public Payload early() { Payload v = new Payload(); if (System.currentTimeMillis() > 0) return v; v.setCode(Kind.A.val()); return v; }
    public Payload aliasWrite() { Payload v = new Payload(); Payload alias = v; v.setCode(Kind.A.val()); alias.setCode(Integer.parseInt(System.getenv("CODE"))); return v; }
    public Payload unusedConstructor() { Payload scratch = new Payload(Kind.A.code); return unknownObject(); }
    public Payload unusedSetter() { Payload scratch = new Payload(); scratch.setCode(Kind.A.code); return unknownObject(); }
    private Payload unknownObject() { return External.load(); }
    public Payload constructor() { return new Payload(Kind.A.code()); }
}
"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", builder),
            ("Facade.java", source),
        ],
        &[
            "builder",
            "conditional",
            "constructor",
            "early",
            "aliasWrite",
            "unusedConstructor",
            "unusedSetter",
        ],
    );
    assert_eq!(
        values(patch(&ir, "builder", "code")),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );
    assert!(values(patch(&ir, "conditional", "code")).contains(&WireValue::Number(0)));
    assert!(values(patch(&ir, "early", "code")).contains(&WireValue::Number(0)));
    assert!(
        patch(&ir, "unusedConstructor", "code")
            .associated_values()
            .is_none()
    );
    assert!(
        patch(&ir, "unusedSetter", "code")
            .associated_values()
            .is_none()
    );
    assert!(
        patch(&ir, "aliasWrite", "code")
            .associated_values()
            .is_none()
    );
    let lombok = "package p;\n@lombok.Data @lombok.AllArgsConstructor public class Payload { private int code; }";
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", lombok),
            ("Facade.java", source),
        ],
        &["constructor"],
    );
    assert_eq!(
        values(patch(&ir, "constructor", "code")),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );
    let rewritten = builder.replace(
        "this.code = code; return this;",
        "this.code = code + 100; return this;",
    );
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", &rewritten),
            ("Facade.java", source),
        ],
        &["builder"],
    );
    assert!(patch(&ir, "builder", "code").associated_values().is_none());
}

#[test]
fn lookup_variants_associate_inputs_without_confusing_output_projection() {
    let kind = r#"package p;
import java.util.Arrays;
import java.util.Collections;
import java.util.Map;
import java.util.HashMap;
import java.util.stream.Collectors;
import java.util.function.Function;
public enum Kind {
    A(1), B(2);
    private final int code;
    Kind(int code) { this.code = code; }
    public int val() { return code; }
    private static final Map<Integer, Kind> INDEX = Arrays.stream(values()).collect(Collectors.toMap(Kind::val, Function.identity()));
    private static final Map<String, Kind> NAMES;
    static {
        Map<String, Kind> names = new HashMap<>();
        for (Kind item : values()) { names.put(item.name(), item); }
        NAMES = Collections.unmodifiableMap(names);
    }
    public static Kind fromMap(int input) { return INDEX.getOrDefault(input, A); }
    public static Kind fromName(String input) { return NAMES.get(input); }
    public static Kind fromAlias(int input) { Kind[] kinds = values(); for (Kind item : kinds) { if (item.val() == input) return item; } return null; }
    public static Kind convert(int input) { switch (input) { case 200: return A; case 300: return B; default: return null; } }
}
"#;
    let dto = "package p;\npublic class Source { private Integer code; private String token; public Integer getCode() { return code; } public String getToken() { return token; } }";
    let source = r#"package p;
public class Facade {
    public Payload map(Source source) { Kind.fromMap(source.getCode()); Payload v = new Payload(); v.setCode(source.getCode()); return v; }
    public Payload alias(Source source) { Kind.fromAlias(source.getCode()); Payload v = new Payload(); v.setCode(source.getCode()); return v; }
    public Payload name(Source source) { Kind.fromName(source.getToken()); Payload v = new Payload(); v.setToken(source.getToken()); return v; }
    public Payload convert(Source source) { Kind.convert(source.getCode()); Payload v = new Payload(); v.setCode(source.getCode()); v.setToken(Kind.convert(source.getCode()).name()); return v; }
}
"#;
    let files = [
        ("Kind.java", kind),
        ("Payload.java", PAYLOAD),
        ("Source.java", dto),
        ("Facade.java", source),
    ];
    let ir = contract(&files, &["map", "alias", "name", "convert"]);
    for method in ["map", "alias"] {
        let patch = patch(&ir, method, "code");
        assert_eq!(
            values(patch),
            vec![WireValue::Number(1), WireValue::Number(2)]
        );
        assert_eq!(
            patch.status,
            ProvenanceStatus::Known,
            "unvalidated input stays open"
        );
    }
    assert_eq!(
        values(patch(&ir, "name", "token")),
        vec![WireValue::String("A".into()), WireValue::String("B".into())]
    );
    assert!(patch(&ir, "convert", "code").associated_values().is_none());
    assert_eq!(
        values(patch(&ir, "convert", "token")),
        vec![WireValue::String("A".into()), WireValue::String("B".into())]
    );
    let mutated = kind.replace(
        "return INDEX.getOrDefault(input, A);",
        "INDEX.put(999, A); return INDEX.getOrDefault(input, A);",
    );
    let ir = contract(
        &[
            ("Kind.java", &mutated),
            ("Payload.java", PAYLOAD),
            ("Source.java", dto),
            ("Facade.java", source),
        ],
        &["map"],
    );
    assert!(patch(&ir, "map", "code").associated_values().is_none());
}

#[test]
fn copy_frameworks_respect_sources_ignores_conversions_and_overwrites() {
    let dto = r#"package p;
public class Source {
    private Integer code;
    private Integer alternate;
    public void setCode(Integer code) { this.code = code; }
    public void setAlternate(Integer alternate) { this.alternate = alternate; }
    public Integer getCode() { return code; }
    public Integer getAlternate() { return alternate; }
}
"#;
    let mapper = r#"package p;
import org.mapstruct.Mapper;
import org.mapstruct.Mapping;
@Mapper public interface Converter {
    Payload map(Source source);
    @Mapping(target="code", source="alternate") Payload rename(Source source);
    @Mapping(target="code", ignore=true) Payload ignore(Source source);
    @Mapping(target="code", expression="java(source.getCode() + 1)") Payload custom(Source source);
}
"#;
    let source = r#"package p;
import org.springframework.beans.BeanUtils;
public class Facade {
    private Converter converter;
    public Payload bean() { Source s = source(); Payload v = new Payload(); BeanUtils.copyProperties(s, v); return v; }
    public Payload ignored() { Source s = source(); Payload v = new Payload(); BeanUtils.copyProperties(s, v, "code"); return v; }
    public Payload overwritten() { Source s = source(); Payload v = new Payload(); BeanUtils.copyProperties(s, v); v.setCode(Integer.parseInt(System.getenv("CODE"))); return v; }
    public Payload map() { Source s = source(); return converter.map(s); }
    public Payload rename() { Source s = source(); return converter.rename(s); }
    public Payload ignore() { Source s = source(); return converter.ignore(s); }
    public Payload custom() { Source s = source(); return converter.custom(s); }
    private Source source() { Source s = new Source(); s.setCode(Kind.A.getCode()); s.setAlternate(Kind.A.val()); return s; }
}
"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Source.java", dto),
            ("Converter.java", mapper),
            ("Facade.java", source),
        ],
        &[
            "bean",
            "ignored",
            "overwritten",
            "map",
            "rename",
            "ignore",
            "custom",
        ],
    );
    for method in ["bean", "map", "rename"] {
        assert_eq!(
            values(patch(&ir, method, "code")),
            vec![WireValue::Number(1), WireValue::Number(2)],
            "{method}"
        );
    }
    for method in ["ignored", "overwritten", "ignore", "custom"] {
        assert!(
            patch(&ir, method, "code").associated_values().is_none(),
            "{method}"
        );
    }
    let shadow = source.replace("import org.springframework.beans.BeanUtils;", "");
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Source.java", dto),
            ("Converter.java", mapper),
            ("Facade.java", &shadow),
            (
                "BeanUtils.java",
                "package p; class BeanUtils { static void copyProperties(Source s, Payload v) { v.setCode(100); } }",
            ),
        ],
        &["bean"],
    );
    assert!(patch(&ir, "bean", "code").associated_values().is_none());
}

#[test]
fn all_eighteen_audit_probes_have_expected_generated_members() {
    let facade = include_str!("../../tests/fixtures/enum-provenance/ProbeController.java")
        .replace("ProbeController", "Facade");
    let files = [
        ("Facade.java", facade.as_str()),
        (
            "Kind.java",
            include_str!("../../tests/fixtures/enum-provenance/Kind.java"),
        ),
        (
            "ConstKind.java",
            include_str!("../../tests/fixtures/enum-provenance/ConstKind.java"),
        ),
        (
            "Payload.java",
            include_str!("../../tests/fixtures/enum-provenance/Payload.java"),
        ),
        (
            "DefaultPayload.java",
            include_str!("../../tests/fixtures/enum-provenance/DefaultPayload.java"),
        ),
        (
            "Source.java",
            include_str!("../../tests/fixtures/enum-provenance/Source.java"),
        ),
    ];
    let methods = [
        "getter",
        "name",
        "val",
        "codeMethod",
        "directEnumField",
        "helper",
        "literalExtra",
        "optional",
        "methodReference",
        "directWrite",
        "constructor",
        "builder",
        "collection",
        "enumField",
        "loopLookup",
        "mapLookup",
        "enumConstantArgument",
        "fieldInitializer",
    ];
    let ir = contract(&files, &methods);
    for method in methods {
        let field = match method {
            "name" => "token",
            "collection" => "codes",
            "enumField" => "kind",
            _ => "code",
        };
        let expected = match method {
            "name" | "enumField" => {
                vec![WireValue::String("A".into()), WireValue::String("B".into())]
            }
            "literalExtra" | "optional" | "fieldInitializer" => vec![
                WireValue::Number(1),
                WireValue::Number(2),
                WireValue::Number(99),
            ],
            _ => vec![WireValue::Number(1), WireValue::Number(2)],
        };
        assert_eq!(values(patch(&ir, method, field)), expected, "{method}");
    }
    let mut config = crate::typescript::tests::config();
    config.backend.contract_roots = vec!["src/main/java/p".into()];
    let typescript = crate::typescript::generate(&ir, &config).unwrap();
    assert_eq!(typescript.enum_files.len(), 4);
    assert!(
        typescript
            .type_files
            .iter()
            .any(|file| typescript.files[file].contains("codes: KindCode[]"))
    );
    let openapi = crate::openapi::generate(&ir, &config).unwrap();
    let document: serde_json::Value = serde_json::from_str(&openapi.source).unwrap();
    assert_eq!(document["paths"].as_object().unwrap().len(), 18);
}

#[test]
fn unresolved_sources_transforms_and_conflicting_enums_never_narrow() {
    let facade = r#"package p;
import missing.ExpressCompany;
import static p.Numbers.ONE;
public class Facade {
    public Payload missing() { return new Payload(ExpressCompany.SF.code()); }
    public Payload mixed() { Payload v = new Payload(); v.setCode(flag() ? Kind.A.code : Other.A.code); return v; }
    public Payload dynamic() { Payload v = new Payload(); v.setToken(flag() ? Kind.A.name() : System.getenv("MARKET")); return v; }
    public Payload statics() { Payload v = new Payload(); v.setCode(flag() ? Kind.A.code : ONE); return v; }
    public Payload cycle() { Payload v = new Payload(); v.setCode(flag() ? Kind.A.code : Cycle.A); return v; }
    boolean flag() { return Boolean.getBoolean("flag"); }
}
enum Other { A(1), B(2); final int code; Other(int code) { this.code = code; } }
class Cycle { static final int A = B; static final int B = A; }
"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Facade.java", facade),
        ],
        &["missing", "mixed", "dynamic", "statics", "cycle"],
    );
    for (method, field) in [
        ("missing", "code"),
        ("mixed", "code"),
        ("dynamic", "token"),
        ("cycle", "code"),
    ] {
        assert!(
            patch(&ir, method, field).associated_values().is_none(),
            "{method}"
        );
    }
    assert_eq!(
        values(patch(&ir, "statics", "code")),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );
    assert!(crate::semantic_diagnostics(&ir).iter().any(|diagnostic| {
        diagnostic["operationKey"] == "Facade#missing"
            && diagnostic["message"]
                .as_str()
                .unwrap_or_default()
                .contains("ExpressCompany")
    }));
    let broken = KIND.replace("Numbers.ONE", "MissingNumber.ONE");
    let ir = contract(
        &[
            ("Kind.java", &broken),
            ("Payload.java", PAYLOAD),
            ("Facade.java", facade),
        ],
        &["statics"],
    );
    assert!(patch(&ir, "statics", "code").associated_values().is_none());
    assert!(
        crate::semantic_diagnostics(&ir)
            .iter()
            .any(|diagnostic| diagnostic["level"] == "warning")
    );
}

#[test]
fn same_name_nested_receivers_never_share_enum_domains() {
    let one = "package p;\npublic class One { public static class MarketVo { private String token; public void setToken(String token) { this.token = token; } } }";
    let two = one.replace("class One", "class Two");
    let facade = r#"package p;
public class Facade {
    public One.MarketVo query() {
        One.MarketVo v = new One.MarketVo();
        Two.MarketVo scratch = new Two.MarketVo();
        scratch.setToken("UNRELATED");
        v.setToken(Kind.A.name());
        return v;
    }
}

"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("One.java", one),
            ("Two.java", &two),
            ("Facade.java", facade),
        ],
        &["query"],
    );
    assert_eq!(
        values(patch(&ir, "query", "token")),
        vec![WireValue::String("A".into()), WireValue::String("B".into())]
    );
}

#[test]
fn custom_setters_and_mapper_lifecycle_keep_unknown_values_visible() {
    let setter = PAYLOAD.replace("this.code = code;", "this.code = code + 100;");
    let facade = "package p;\npublic class Facade { public Payload query() { Payload v = new Payload(); v.setCode(Kind.A.code); return v; } }";
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", &setter),
            ("Facade.java", facade),
        ],
        &["query"],
    );
    assert!(patch(&ir, "query", "code").associated_values().is_none());
    let mapper = r#"package p;
import org.mapstruct.Mapper;
import org.mapstruct.AfterMapping;
import org.mapstruct.MappingTarget;
@Mapper public interface Converter {
    Payload map(Source source);
    @AfterMapping default void rewrite(@MappingTarget Payload v) { v.setCode(100); }
}
"#;
    let source = "package p;\npublic class Source { private int code; public void setCode(int code) { this.code = code; } public int getCode() { return code; } }";
    let facade = "package p;\npublic class Facade { private Converter converter; public Payload query() { Source s = new Source(); s.setCode(Kind.A.code); return converter.map(s); } }";
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Source.java", source),
            ("Converter.java", mapper),
            ("Facade.java", facade),
        ],
        &["query"],
    );
    assert!(patch(&ir, "query", "code").associated_values().is_none());
}

#[test]
fn reverse_maps_and_generic_parsers_keep_call_site_domains() {
    let kind = r#"package p;
import java.util.Map;
import java.util.HashMap;
import java.util.List;
import java.util.ArrayList;
import java.util.Collections;
public enum Kind {
    A(1), B(2);
    private final int code;
    Kind(int code) { this.code = code; }
    public int getCode() { return code; }
    private static final Map<Integer, Kind> CODES;
    private static final Map<String, Kind> NAMES;
    static {
        Map<Integer, Kind> codes = new HashMap<>();
        Map<String, Kind> names = new HashMap<>();
        List<Integer> selected = new ArrayList<>();
        for (Kind item : values()) {
            codes.put(item.getCode(), item);
            names.put(item.name(), item);
            if (item.getCode() > 1) selected.add(item.getCode());
        }
        CODES = Collections.unmodifiableMap(codes);
        NAMES = Collections.unmodifiableMap(names);
    }
    public static Kind ofCode(Integer code) { return CODES.get(code); }
    public static Kind ofName(String name) { return NAMES.get(name); }
}
enum Other { X, Y; public static Other ofName(String name) { for (Other item : values()) { if (item.name().equals(name)) return item; } return null; } }
"#;
    let request = "package p;\nimport java.util.List;\npublic class Source { private Integer code; private List<String> names; private List<String> otherNames; public Integer getCode() { return code; } public List<String> getNames() { return names; } public List<String> getOtherNames() { return otherNames; } }";
    let facade = r#"package p;
import java.util.List;
import java.util.ArrayList;
import java.util.function.Function;
public class Facade {
    public Payload query(Source request) {
        Kind parsed = Kind.ofCode(request.getCode());
        if (parsed == null) throw new IllegalArgumentException();
        convert(request.getNames(), Kind::ofName, Kind::getCode);
        convert(request.getOtherNames(), Other::ofName, Other::name);
        Payload result = new Payload(); result.setCode(parsed.getCode()); return result;
    }
    public <E,C> List<C> convert(List<String> names, Function<String,E> parser, Function<E,C> getter) {
        List<C> result = new ArrayList<>();
        for (String name : names) {
            E value = parser.apply(name);
            if (value == null) continue;
            result.add(getter.apply(value));
        }
        return result;
    }
}
"#;
    let files = [
        ("Kind.java", kind),
        ("Payload.java", PAYLOAD),
        ("Source.java", request),
        ("Facade.java", facade),
    ];
    let ir = contract(&files, &["query"]);
    let input = |ir: &ContractIr, name: &str| {
        ir.operations[0]
            .semantic_patches
            .iter()
            .find(|patch| {
                patch.target.source == FieldSource::Request && patch.target.field_name == name
            })
            .unwrap()
            .clone()
    };
    assert_eq!(
        values(&input(&ir, "code")),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );
    assert_eq!(
        values(&input(&ir, "names")),
        vec![WireValue::String("A".into()), WireValue::String("B".into())]
    );
    assert_eq!(
        values(&input(&ir, "otherNames")),
        vec![WireValue::String("X".into()), WireValue::String("Y".into())]
    );
    assert_eq!(
        input(&ir, "names").status,
        ProvenanceStatus::Known,
        "skipping unknown names does not reject the request"
    );
    for replacement in [
        "name = System.getenv(\"NAME\"); E value = parser.apply(name);",
        "E value = Kind.ofName(\"A\");",
    ] {
        let changed = facade.replace("E value = parser.apply(name);", replacement);
        let ir = contract(
            &[
                ("Kind.java", kind),
                ("Payload.java", PAYLOAD),
                ("Source.java", request),
                ("Facade.java", &changed),
            ],
            &["query"],
        );
        assert!(input(&ir, "names").associated_values().is_none());
    }
}

#[test]
fn dropdown_name_union_keeps_only_selected_foreign_constants() {
    let facade = r#"package p;
import java.util.List;
import java.util.ArrayList;
public class Facade {
    public List<Payload> query() {
        List<Payload> result = new ArrayList<>();
        Payload unlocked = new Payload(); unlocked.setToken(Kind.A.name());
        result.add(unlocked);
        for (Other item : Other.values()) { Payload value = new Payload(); value.setToken(item.name()); result.add(value); }
        return result;
    }
    public Payload labels() {
        Payload value = new Payload(); value.setToken(Kind.A.getDesc()); value.setToken(Other.X.getDesc()); return value;
    }
    public Payload constantUnion() {
        Payload value = new Payload(); value.setToken(Extra.LEFT.name()); value.setToken(Extra.RIGHT.name()); value.setToken(Kind.A.name()); return value;
    }
    public Payload reverseUnion() {
        Payload value = new Payload(); value.setToken(Kind.A.name()); value.setToken(Extra.RIGHT.name()); value.setToken(Extra.LEFT.name()); return value;
    }
}
enum Other { X("Extra"), Y("Other"); final String desc; Other(String desc) { this.desc=desc; } String getDesc() { return desc; } }
enum Extra { LEFT, RIGHT, UNUSED }
"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Facade.java", facade),
        ],
        &["query", "labels", "constantUnion", "reverseUnion"],
    );
    assert_eq!(
        values(patch(&ir, "query", "token")),
        vec![
            WireValue::String("A".into()),
            WireValue::String("X".into()),
            WireValue::String("Y".into())
        ]
    );
    assert!(
        patch(&ir, "query", "token")
            .evidence
            .iter()
            .any(|item| item.contains("p.Kind#name + p.Other#name"))
    );
    for method in ["constantUnion", "reverseUnion"] {
        let actual = values(patch(&ir, method, "token"));
        assert_eq!(actual.len(), 3, "{method}: {actual:?}");
        for expected in ["A", "LEFT", "RIGHT"] {
            assert!(
                actual.contains(&WireValue::String(expected.into())),
                "{method}: {actual:?}"
            );
        }
    }
    let label = patch(&ir, "labels", "token");
    assert!(label.associated_values().is_none());
    assert_eq!(
        label.enum_candidate.as_ref().unwrap().status,
        crate::model::EnumCandidateStatus::Ignored
    );
}

#[test]
fn nested_collection_copies_follow_rpc_objects_and_callbacks() {
    let source = "package p;\npublic class Source { private String token; public String getToken() { return token; } public void setToken(String token) { this.token=token; } }";
    let holder = "package p;\nimport java.util.List;\npublic class Holder { private List<Source> sources; public List<Source> getSources() { return sources; } public void setSources(List<Source> sources) { this.sources=sources; } }";
    let wrapper = "package p;\npublic class ApiResult<T> { private T data; public T getData() { return data; } public static <T> ApiResult<T> success(T data) { return null; } }";
    let facade = r#"package p;
import java.util.List;
import java.util.ArrayList;
import java.util.Map;
import java.util.HashMap;
import java.util.stream.Collectors;
public class Facade {
    public List<Payload> loop() {
        ApiResult<Holder> result = remote();
        Holder holder = null;
        if (result != null) holder = result.getData();
        List<Payload> values = new ArrayList<>();
        for (Source item : holder.getSources()) { values.add(convert(item)); }
        return values;
    }
    public List<Payload> stream() {
        Holder holder = remote().getData();
        return holder.getSources().stream().map(this::convert).collect(Collectors.toList());
    }
    public List<Payload> mapValues() {
        Map<Integer, Source> map = new HashMap<>();
        Source source = new Source(); source.setToken(Kind.A.name()); map.put(1, source);
        List<Payload> values = new ArrayList<>();
        for (Source item : map.values()) { values.add(convert(item)); }
        return values;
    }
    public List<Payload> unknown() {
        remote(); Holder holder = missing();
        return holder.getSources().stream().map(this::convert).collect(Collectors.toList());
    }
    public List<Payload> rewritten() {
        Holder holder = remote().getData();
        List<Payload> values = new ArrayList<>();
        for (Source item : holder.getSources()) { item = external(); values.add(convert(item)); }
        return values;
    }
    private Payload convert(Source source) { Payload value = new Payload(); value.setToken(source.getToken()); return value; }
    private ApiResult<Holder> remote() { ApiResult<Holder> result = nested(); return result; }
    private ApiResult<Holder> nested() {
        Holder holder = new Holder(); List<Source> sources = new ArrayList<>();
        Source source = new Source(); source.setToken(Kind.A.name()); sources.add(source);
        holder.setSources(sources); return ApiResult.success(holder);
    }
    private Holder missing() { return null; }
    private Source external() { return null; }
}
"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Source.java", source),
            ("Holder.java", holder),
            ("ApiResult.java", wrapper),
            ("Facade.java", facade),
        ],
        &["loop", "stream", "mapValues", "unknown", "rewritten"],
    );
    for method in ["loop", "stream", "mapValues"] {
        assert_eq!(
            values(patch(&ir, method, "token")),
            vec![WireValue::String("A".into()), WireValue::String("B".into())],
            "{method}"
        );
    }
    for method in ["unknown", "rewritten"] {
        assert!(
            patch(&ir, method, "token").associated_values().is_none(),
            "{method}"
        );
    }
    for transformed in [
        source.replace("this.token=token;", "this.token=token + \"-changed\";"),
        source.replace("return token;", "return token + \"-changed\";"),
    ] {
        let ir = contract(
            &[
                ("Kind.java", KIND),
                ("Payload.java", PAYLOAD),
                ("Source.java", &transformed),
                ("Holder.java", holder),
                ("ApiResult.java", wrapper),
                ("Facade.java", facade),
            ],
            &["loop"],
        );
        assert!(patch(&ir, "loop", "token").associated_values().is_none());
    }
    for escaped in [
        facade.replace(
            "holder.setSources(sources);",
            "mutate(sources); holder.setSources(sources);",
        ),
        facade.replace(
            "holder.setSources(sources);",
            "List<Source> alias = sources; alias.add(external()); holder.setSources(sources);",
        ),
    ] {
        let ir = contract(
            &[
                ("Kind.java", KIND),
                ("Payload.java", PAYLOAD),
                ("Source.java", source),
                ("Holder.java", holder),
                ("ApiResult.java", wrapper),
                ("Facade.java", &escaped),
            ],
            &["loop"],
        );
        assert!(patch(&ir, "loop", "token").associated_values().is_none());
    }
}

#[test]
fn dynamic_map_configuration_and_composed_details_remain_scalar() {
    let config = "package p;\npublic class Config { private String displayName; public String getDisplayName() { return displayName; } }";
    let facade = r#"package p;
import java.util.Map;
import java.util.List;
import java.util.stream.Collectors;
public class Facade {
    public Payload configuration() { Payload value = new Payload(); Map.Entry<String,Config> entry = null; value.setToken(label(entry)); return value; }
    private String label(Map.Entry<String,Config> entry) { return entry.getValue().getDisplayName() == null ? Kind.A.name() : entry.getValue().getDisplayName(); }
    public Payload detail() { Payload value = new Payload(); value.setToken(compose(Kind.A.getDesc(), System.getenv("DETAIL"))); return value; }
    private String compose(String label, String detail) { if (detail == null) return label; return label + "：【" + detail + "】"; }
    public Payload joining() { Payload value = new Payload(); value.setToken(List.of(Kind.A.name(), Kind.B.name()).stream().collect(Collectors.joining(";"))); return value; }
}
"#;
    let ir = contract(
        &[
            ("Kind.java", KIND),
            ("Payload.java", PAYLOAD),
            ("Config.java", config),
            ("Facade.java", facade),
        ],
        &["configuration", "detail", "joining"],
    );
    let configuration = patch(&ir, "configuration", "token");
    assert!(configuration.associated_values().is_none());
    assert!(
        configuration
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("p.Config#displayName"),
        "{configuration:#?}"
    );
    for method in ["detail", "joining"] {
        assert!(
            patch(&ir, method, "token").associated_values().is_none(),
            "{method}"
        );
    }
}
