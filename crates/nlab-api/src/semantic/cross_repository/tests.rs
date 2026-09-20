use super::*;
use crate::graph::test_snapshot;
use crate::model::{ContractIr, TargetIdentity};
use crate::semantic::tests::{call, contains, node, write};

#[test]
fn see_references_respect_imports_and_never_replace_code_evidence() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "DTO.java",
        "package response;\nimport correct.EState;\nclass DTO { Integer code; }",
    );
    write(
        root.path(),
        "Missing.java",
        "package response;\nimport missing.EState;\nclass Missing {}",
    );
    write(
        root.path(),
        "Correct.java",
        "package correct; enum EState { A(1), B(2); final int code; EState(int code) { this.code=code; } }",
    );
    write(
        root.path(),
        "Wrong.java",
        "package wrong; enum EState { X(8), Y(9); final int code; EState(int code) { this.code=code; } }",
    );
    let mut graph = test_snapshot(
        vec![
            node("dto", "class", "DTO", "response::DTO", "DTO.java", 3, ""),
            node(
                "field",
                "field",
                "code",
                "response::DTO::code",
                "DTO.java",
                3,
                "Integer code",
            ),
            node(
                "missing",
                "class",
                "Missing",
                "response::Missing",
                "Missing.java",
                3,
                "",
            ),
            node(
                "correct",
                "enum",
                "EState",
                "correct::EState",
                "Correct.java",
                1,
                "",
            ),
            node(
                "wrong",
                "enum",
                "EState",
                "wrong::EState",
                "Wrong.java",
                1,
                "",
            ),
        ],
        vec![contains("dto", "field")],
    );
    for description in [
        "@see EState",
        "@see\tEState",
        "@see\nEState",
        "@see EState#A",
        "@see correct.EState",
    ] {
        graph.nodes.get_mut("field").unwrap().docstring = Some(description.to_owned());
        let project = JavaProject::load(root.path(), &graph).unwrap();
        assert_eq!(
            linked_enum_nodes(&project, "DTO.java", "response.DTO", description)[0].id,
            "correct"
        );
        let analyzer = SemanticAnalyzer::new(&project);
        let mut hint = Domain::default();
        assert!(analyzer.copied_field_enum_reference(&graph.nodes["dto"], "code", &mut hint));
        assert!(hint.enum_fqn.is_none() && hint.values.is_empty());
        assert!(
            hint.unknown
                .iter()
                .any(|reason| reason.contains("unverified @see"))
        );
        let mut actual = Domain {
            enum_fqn: Some("wrong.EState".to_owned()),
            ..Domain::default()
        };
        analyzer.copied_field_enum_reference(&graph.nodes["dto"], "code", &mut actual);
        assert_eq!(actual.enum_fqn.as_deref(), Some("wrong.EState"));
        assert!(actual.unknown.is_empty());
        assert!(
            actual
                .evidence
                .iter()
                .any(|item| item.ends_with("conflicts-with-code"))
        );
        let mut schemas = BTreeMap::from([(
            "response.DTO".to_owned(),
            Schema {
                fqn: "response.DTO".to_owned(),
                name: "DTO".to_owned(),
                source_path: "DTO.java".to_owned(),
                description: None,
                type_parameters: vec![],
                fields: vec![crate::model::Field {
                    name: "code".to_owned(),
                    java_type: parse_java_type("Integer").unwrap(),
                    optional: false,
                    description: Some(description.to_owned()),
                    declared_values: None,
                    linked_enum: None,
                }],
            },
        )]);
        analyzer.enrich_linked_enums(&mut schemas).unwrap();
        assert!(schemas["response.DTO"].fields[0].linked_enum.is_none());
        assert!(
            linked_enum_nodes(&project, "Missing.java", "response.Missing", "@see EState")
                .is_empty()
        );
        assert!(
            linked_enum_nodes(&project, "DTO.java", "response.DTO", "@see response.DTO").is_empty()
        );
        assert_eq!(
            linked_enum_nodes(
                &project,
                "DTO.java",
                "response.DTO",
                "@see correct.EState\n@see wrong.EState"
            )
            .len(),
            2
        );
    }
}

#[test]
fn database_value_lookup_exports_known_members_without_closing_the_field() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "Code.java",
        "package p;\nenum Code {\nA(1), B(2);\nfinal int code;\nCode(int code) { this.code=code; }\nint getCode() { return code; }\nstatic Code fromCode(int code) {\nfor (Code item : values()) { if (item.getCode() == code) { return item; } }\nreturn null;\n}\nstatic String description(int code) { Code value=fromCode(code); return value == null ? \"\" : value.name(); }\n}\n",
    );
    write(
        root.path(),
        "Service.java",
        "package p;\nclass Service {\nvoid copy(Source source) { Code.description(source.getCode()); }\n}\n",
    );
    let graph = test_snapshot(
        vec![
            node("enum", "enum", "Code", "p::Code", "Code.java", 2, ""),
            node(
                "description",
                "method",
                "description",
                "p::Code::description",
                "Code.java",
                11,
                "String (int code)",
            ),
            node(
                "lookup",
                "method",
                "fromCode",
                "p::Code::fromCode",
                "Code.java",
                7,
                "Code (int code)",
            ),
            node(
                "service",
                "class",
                "Service",
                "p::Service",
                "Service.java",
                2,
                "",
            ),
            node(
                "copy",
                "method",
                "copy",
                "p::Service::copy",
                "Service.java",
                3,
                "void (Source source)",
            ),
        ],
        vec![
            contains("enum", "lookup"),
            contains("enum", "description"),
            contains("service", "copy"),
        ],
    );
    let project = JavaProject::load(root.path(), &graph).unwrap();
    let mut analyzer = SemanticAnalyzer::new(&project);
    analyzer.index_enum_lookups().unwrap();
    let domain = analyzer
        .lookup_field_domain(&graph.nodes["copy"], "source", "getCode", usize::MAX)
        .unwrap()
        .unwrap();
    let patch = classify_patch(
        FieldTarget {
            source: FieldSource::Response,
            operation_key: "query".to_owned(),
            schema_fqn: "p.DTO".to_owned(),
            field_path: "code".to_owned(),
            field_name: "code".to_owned(),
        },
        vec![domain],
    );
    assert_eq!(patch.status, ProvenanceStatus::Known);
    assert!(patch.values.is_empty());
    assert_eq!(patch.known_values.len(), 2);
    assert!(patch.enum_associated);
    assert!(
        analyzer
            .lookup_field_domain(&graph.nodes["copy"], "source", "getState", usize::MAX)
            .unwrap()
            .is_none()
    );
}

#[test]
fn explicit_imports_and_duplicate_full_names_never_select_a_different_repository() {
    let primary = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write(
        primary.path(),
        "Caller.java",
        "package p;\nimport wrong.Code;\nclass Caller {}\n",
    );
    let mut graph = test_snapshot(
        vec![node(
            "caller",
            "class",
            "Caller",
            "p::Caller",
            "Caller.java",
            3,
            "",
        )],
        vec![],
    );
    for root in [first.path(), second.path()] {
        write(root, "Code.java", "package dep;\nenum Code { A; }\n");
        graph
            .include_repository(
                root,
                test_snapshot(
                    vec![node(
                        "enum",
                        "enum",
                        "Code",
                        "dep::Code",
                        "Code.java",
                        2,
                        "",
                    )],
                    vec![],
                ),
            )
            .unwrap();
        let project = JavaProject::load(primary.path(), &graph).unwrap();
        assert!(
            project
                .resolve_type("Caller.java", "p.Caller", &parse_java_type("Code").unwrap())
                .is_none()
        );
        assert!(
            linked_enum_nodes(&project, "Caller.java", "p.Caller", "{@link wrong.Code}").is_empty()
        );
    }
    let project = JavaProject::load(primary.path(), &graph).unwrap();
    assert!(project.node_for_fqn("dep.Code").is_none());
    assert!(linked_enum_nodes(&project, "Caller.java", "p.Caller", "{@link dep.Code}").is_empty());
}

#[test]
fn remote_request_validation_and_copied_response_generate_without_false_narrowing() {
    for (caught, bound) in [(false, true), (true, true), (false, false)] {
        let primary = tempfile::tempdir().unwrap();
        let dependency = tempfile::tempdir().unwrap();
        let facade = "contract/src/main/java/p/contract/IFacade.java";
        write(
            primary.path(),
            facade,
            "package p.contract;\nimport p.DTO;\nimport p.Request;\n@ServiceContract public interface IFacade { DTO query(Request req); }\n",
        );
        write(
            primary.path(),
            "DTO.java",
            "package p;\nclass DTO {\nString code;\nvoid setCode(String code) { this.code = code; }\n}\n",
        );
        write(
            primary.path(),
            "Request.java",
            "package p;\nclass Request {\nString mode;\nString getMode() { return mode; }\n}\n",
        );
        let invocation = if caught {
            "Result result; try { result = remote.query(req); } catch(Exception e) { return new DTO(); }"
        } else {
            "Result result = remote.query(req);"
        };
        write(
            primary.path(),
            "Facade.java",
            &format!(
                "package p;\nimport dep.IRemote;\nimport dep.Result;\nclass Facade {{\nIRemote remote;\nDTO query(Request req) {{\n{invocation}\nDTO dto = new DTO();\ndto.setCode(result.getCode());\nreturn dto;\n}}\n}}\n"
            ),
        );
        let mut graph = test_snapshot(
            vec![
                node(
                    "facade",
                    "interface",
                    "IFacade",
                    "p.contract::IFacade",
                    facade,
                    4,
                    "",
                ),
                node(
                    "root",
                    "method",
                    "query",
                    "p.contract::IFacade::query",
                    facade,
                    4,
                    "DTO (Request req)",
                ),
                node("impl", "class", "Facade", "p::Facade", "Facade.java", 4, ""),
                node(
                    "remote",
                    "field",
                    "remote",
                    "p::Facade::remote",
                    "Facade.java",
                    5,
                    "IRemote remote",
                ),
                node(
                    "query",
                    "method",
                    "query",
                    "p::Facade::query",
                    "Facade.java",
                    6,
                    "DTO (Request req)",
                ),
                node("dto", "class", "DTO", "p::DTO", "DTO.java", 2, ""),
                node(
                    "field",
                    "field",
                    "code",
                    "p::DTO::code",
                    "DTO.java",
                    3,
                    "String code",
                ),
                node(
                    "setter",
                    "method",
                    "setCode",
                    "p::DTO::setCode",
                    "DTO.java",
                    4,
                    "void (String code)",
                ),
                node(
                    "request",
                    "class",
                    "Request",
                    "p::Request",
                    "Request.java",
                    2,
                    "",
                ),
                node(
                    "mode",
                    "field",
                    "mode",
                    "p::Request::mode",
                    "Request.java",
                    3,
                    "String mode",
                ),
                node(
                    "get-mode",
                    "method",
                    "getMode",
                    "p::Request::getMode",
                    "Request.java",
                    4,
                    "String ()",
                ),
            ],
            vec![
                contains("facade", "root"),
                contains("impl", "query"),
                contains("impl", "remote"),
                contains("dto", "field"),
                contains("dto", "setter"),
                contains("request", "mode"),
                contains("request", "get-mode"),
                GraphEdge {
                    kind: "implements".to_owned(),
                    ..contains("impl", "facade")
                },
                call("query", "setter", 9),
            ],
        );
        write(
            dependency.path(),
            "IRemote.java",
            "package dep;\nimport p.Request;\ninterface IRemote {\nResult query(Request req);\n}\n",
        );
        write(
            dependency.path(),
            "Remote.java",
            "package dep;\nimport p.Request;\nclass Remote {\nResult query(Request req) {\nObjects.requireNonNull(Code.fromCode(req.getMode()));\nResult result = new Result();\nresult.setCode(Code.A.getCode());\nreturn result;\n}\n}\n",
        );
        write(
            dependency.path(),
            "Result.java",
            "package dep;\nclass Result {\nString code;\nvoid setCode(String code) { this.code = code; }\nString getCode() { return code; }\n}\n",
        );
        write(
            dependency.path(),
            "Code.java",
            "package dep;\nenum Code {\nA(\"a\", \"甲\"), B(\"b\", \"乙\");\nfinal String code;\nfinal String name;\nCode(String code, String name) { this.code=code; this.name=name; }\nString getCode() { return code; }\nstatic Code fromCode(String value) {\nfor (Code item : values()) { if (item.getCode().equals(value)) { return item; } }\nthrow new IllegalArgumentException();\n}\n}\n",
        );
        let remote = test_snapshot(
            vec![
                node(
                    "facade",
                    "interface",
                    "IRemote",
                    "dep::IRemote",
                    "IRemote.java",
                    3,
                    "",
                ),
                node(
                    "root",
                    "method",
                    "query",
                    "dep::IRemote::query",
                    "IRemote.java",
                    4,
                    "Result (Request req)",
                ),
                node(
                    "impl",
                    "class",
                    "Remote",
                    "dep::Remote",
                    "Remote.java",
                    3,
                    "",
                ),
                node(
                    "query",
                    "method",
                    "query",
                    "dep::Remote::query",
                    "Remote.java",
                    4,
                    "Result (Request req)",
                ),
                node(
                    "dto",
                    "class",
                    "Result",
                    "dep::Result",
                    "Result.java",
                    2,
                    "",
                ),
                node(
                    "field",
                    "field",
                    "code",
                    "dep::Result::code",
                    "Result.java",
                    3,
                    "String code",
                ),
                node(
                    "setter",
                    "method",
                    "setCode",
                    "dep::Result::setCode",
                    "Result.java",
                    4,
                    "void (String code)",
                ),
                node(
                    "getter",
                    "method",
                    "getCode",
                    "dep::Result::getCode",
                    "Result.java",
                    5,
                    "String ()",
                ),
                node("enum", "enum", "Code", "dep::Code", "Code.java", 2, ""),
                node(
                    "get-code",
                    "method",
                    "getCode",
                    "dep::Code::getCode",
                    "Code.java",
                    7,
                    "String ()",
                ),
                node(
                    "lookup",
                    "method",
                    "fromCode",
                    "dep::Code::fromCode",
                    "Code.java",
                    8,
                    "Code (String value)",
                ),
            ],
            vec![
                contains("facade", "root"),
                contains("impl", "query"),
                contains("dto", "field"),
                contains("dto", "setter"),
                contains("dto", "getter"),
                contains("enum", "get-code"),
                contains("enum", "lookup"),
                GraphEdge {
                    kind: "implements".to_owned(),
                    ..contains("impl", "facade")
                },
                call("query", "setter", 7),
            ],
        );
        graph.include_repository(dependency.path(), remote).unwrap();
        if bound {
            graph.resolved_external_calls.insert((
                "Facade.java".to_owned(),
                7,
                invocation.find("remote.query").unwrap() + 1,
                "dep.IRemote".to_owned(),
            ));
        }
        assert_eq!(graph.nodes["dto"].qualified_name, "p::DTO");
        let project = JavaProject::load(primary.path(), &graph).unwrap();
        let target = TargetIdentity {
            app_name: "demo".to_owned(),
            branch: "test".to_owned(),
            commit: "test".to_owned(),
            codegraph_version: "test".to_owned(),
            codegraph_extraction_version: "test".to_owned(),
        };
        let roots = vec!["contract/src/main/java/p/contract".to_owned()];
        let (mut operations, schemas) = project.build_contracts(&target, &roots).unwrap();
        SemanticAnalyzer::new(&project)
            .enrich(&mut operations, &schemas)
            .unwrap();
        let patches = &operations[0].semantic_patches;
        let request = patches
            .iter()
            .find(|patch| {
                patch.target.source == FieldSource::Request && patch.target.field_name == "mode"
            })
            .unwrap();
        assert_eq!(
            request.status,
            if !bound {
                ProvenanceStatus::Unresolved
            } else if caught {
                ProvenanceStatus::Known
            } else {
                ProvenanceStatus::Closed
            },
            "{request:#?}"
        );
        let response = patches
            .iter()
            .find(|patch| {
                patch.target.source == FieldSource::Response && patch.target.field_name == "code"
            })
            .unwrap();
        assert_eq!(
            response.status,
            if bound {
                ProvenanceStatus::Known
            } else {
                ProvenanceStatus::Unresolved
            },
            "{response:#?}"
        );
        assert!(response.values.is_empty());
        assert_eq!(response.enum_associated, bound && !caught, "{response:#?}");
        assert_eq!(response.known_values.len(), if bound { 2 } else { 0 });
        if bound {
            assert!(
                response
                    .enum_source
                    .as_ref()
                    .unwrap()
                    .starts_with("__dependencies/")
            );
        }
        let ir = ContractIr {
            target,
            operations,
            schemas,
        };
        let mut config = crate::typescript::tests::config();
        config.backend.contract_roots = roots;
        let generated = crate::typescript::generate(&ir, &config).unwrap();
        assert_eq!(generated.enum_files.len(), if bound { 1 } else { 0 });
        if bound {
            assert!(generated.files[&generated.enum_files[0]].contains("\"a\""));
        }
        let openapi = crate::openapi::generate(&ir, &config).unwrap();
        let openapi: serde_json::Value = serde_json::from_str(&openapi.source).unwrap();
        let mut associated_codes = 0;
        for schema in openapi["components"]["schemas"]
            .as_object()
            .unwrap()
            .values()
        {
            if let Some(code) = schema["properties"].get("code") {
                if bound && code.get("enum").is_some() {
                    associated_codes += 1;
                    assert_eq!(code["enum"], serde_json::json!(["a", "b"]));
                } else {
                    assert!(code.get("enum").is_none());
                }
            }
        }
        assert_eq!(associated_codes > 0, bound && !caught);
        project.verify_sources(primary.path()).unwrap();
        write(dependency.path(), "Code.java", "changed during generation");
        assert!(project.verify_sources(primary.path()).is_err());
    }
}

#[test]
fn association_tracks_same_values_for_lookups_aliases_and_non_description_helpers() {
    for (body, associated) in [
        (
            "int value = source.getCode(); Code.fromCode(source.getCode());",
            true,
        ),
        (
            "int value = source.getCode(); Code.color(\"primary\", value);",
            true,
        ),
        (
            "int value = source.getCode(); Code.fromCode(other.getCode());",
            false,
        ),
        (
            "int value = source.getCode(); Code.fromCode(source.getCode() + 1);",
            false,
        ),
        (
            "int value = source.getCode(); source.setCode(999); Code.fromCode(source.getCode());",
            false,
        ),
        (
            "int value = source.getCode(); value = 999; Code.fromCode(value);",
            false,
        ),
        (
            "int value = source.getCode(); Source alias = source; alias.code++; Code.fromCode(source.getCode());",
            false,
        ),
        (
            "int value = source.getCode(); Source alias = source; alias.code = 9; Code.fromCode(source.getCode());",
            false,
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "Code.java",
            "package p;\nenum Code {\nA(1), B(2);\nfinal int code;\nCode(int code) { this.code=code; }\nint getCode() { return code; }\nstatic Code fromCode(int code) {\nfor (Code item : values()) { if (item.getCode() == code) { return item; } }\nreturn null;\n}\nstatic String color(String theme, int code) { Code item = fromCode(code); return item == null ? theme : item.name(); }\n}\n",
        );
        let source = format!(
            "package p;\nclass Service {{\nvoid copy(Source source, Source other) {{ {body} }}\n}}\n"
        );
        write(root.path(), "Service.java", &source);
        let graph = test_snapshot(
            vec![
                node("enum", "enum", "Code", "p::Code", "Code.java", 2, ""),
                node(
                    "lookup",
                    "method",
                    "fromCode",
                    "p::Code::fromCode",
                    "Code.java",
                    7,
                    "Code (int code)",
                ),
                node(
                    "color",
                    "method",
                    "color",
                    "p::Code::color",
                    "Code.java",
                    11,
                    "String (String theme, int code)",
                ),
                node(
                    "service",
                    "class",
                    "Service",
                    "p::Service",
                    "Service.java",
                    2,
                    "",
                ),
                node(
                    "copy",
                    "method",
                    "copy",
                    "p::Service::copy",
                    "Service.java",
                    3,
                    "void (Source source, Source other)",
                ),
            ],
            vec![
                contains("enum", "lookup"),
                contains("enum", "color"),
                contains("service", "copy"),
            ],
        );
        let project = JavaProject::load(root.path(), &graph).unwrap();
        let mut analyzer = SemanticAnalyzer::new(&project);
        analyzer.index_enum_lookups().unwrap();
        let domain = analyzer
            .lookup_field_domain(
                &graph.nodes["copy"],
                "source",
                "getCode",
                source.find("source.getCode()").unwrap(),
            )
            .unwrap();
        assert!(
            analyzer
                .lookup_field_domain(&graph.nodes["copy"], "provider.next()", "getCode", 0)
                .unwrap()
                .is_none()
        );
        assert_eq!(domain.is_some(), associated, "{body}");
    }
}

#[test]
fn enum_association_rejects_conflicts_transforms_and_unknown_writes_but_keeps_null() {
    let target = FieldTarget {
        source: FieldSource::Response,
        operation_key: "query".to_owned(),
        schema_fqn: "p.DTO".to_owned(),
        field_path: "state".to_owned(),
        field_name: "state".to_owned(),
    };
    let domain = Domain {
        enum_fqn: Some("p.State".to_owned()),
        enum_source: Some("State.java".to_owned()),
        accessor: Some("getCode".to_owned()),
        complete: true,
        values: vec![CodedValue {
            value: WireValue::Number(1),
            key: Some("A".to_owned()),
            label: "A".to_owned(),
        }],
        ..Domain::default()
    };
    let mut raw = domain.clone();
    raw.closure_gaps.insert("read-side enum lookup".to_owned());
    let patch = classify_patch(target.clone(), vec![raw]);
    assert_eq!(patch.status, ProvenanceStatus::Known);
    assert!(patch.enum_associated);
    let nullable = Domain {
        literals: BTreeSet::from(["null".to_owned()]),
        ..Domain::default()
    };
    let patch = classify_patch(target.clone(), vec![domain.clone(), nullable]);
    assert!(patch.enum_associated && patch.nullable);
    for bad in [
        Domain {
            enum_fqn: Some("p.Other".to_owned()),
            ..domain.clone()
        },
        Domain {
            accessor: Some("getOther".to_owned()),
            ..domain.clone()
        },
        Domain {
            unknown: BTreeSet::from(["opaque write".to_owned()]),
            ..Domain::default()
        },
        Domain {
            transformed: true,
            ..Domain::default()
        },
        Domain {
            literals: BTreeSet::from(["999".to_owned()]),
            ..Domain::default()
        },
    ] {
        assert!(!classify_patch(target.clone(), vec![domain.clone(), bad]).enum_associated);
    }
}

#[test]
fn enhanced_for_enum_receivers_keep_their_lexical_scope() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "Code.java",
        "package p;\nenum Code { A, B; }\n",
    );
    let source = "package p;\nclass Service {\nvoid copy() { for (Code item : Code.values()) { item.getCode(); } after(); }\n}\n";
    write(root.path(), "Service.java", source);
    let graph = test_snapshot(
        vec![
            node("enum", "enum", "Code", "p::Code", "Code.java", 2, ""),
            node(
                "service",
                "class",
                "Service",
                "p::Service",
                "Service.java",
                2,
                "",
            ),
            node(
                "copy",
                "method",
                "copy",
                "p::Service::copy",
                "Service.java",
                3,
                "void ()",
            ),
        ],
        vec![contains("service", "copy")],
    );
    let project = JavaProject::load(root.path(), &graph).unwrap();
    let mut analyzer = SemanticAnalyzer::new(&project);
    assert!(
        analyzer
            .enum_for_receiver(
                &graph.nodes["copy"],
                "item",
                source.find("item.getCode").unwrap()
            )
            .unwrap()
            .is_some()
    );
    assert!(
        analyzer
            .enum_for_receiver(
                &graph.nodes["copy"],
                "item",
                source.find("after()").unwrap()
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn abstract_dispatch_keeps_argument_identity_without_same_name_fallback() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "Base.java",
        "package p;\nabstract class Base {\nabstract void render(DTO value);\n}\n",
    );
    write(
        root.path(),
        "Child.java",
        "package p;\nclass Child extends Base {\nvoid render(DTO value) {}\n}\n",
    );
    write(
        root.path(),
        "Other.java",
        "package p;\nclass Other {\nvoid render(DTO value) {}\n}\n",
    );
    write(
        root.path(),
        "Caller.java",
        "package p;\nclass Caller {\nvoid run(Base base, DTO value) { base.render(value); }\n}\n",
    );
    let graph = test_snapshot(
        vec![
            node("base", "class", "Base", "p::Base", "Base.java", 2, ""),
            node(
                "abstract",
                "method",
                "render",
                "p::Base::render",
                "Base.java",
                3,
                "void (DTO value)",
            ),
            node("child", "class", "Child", "p::Child", "Child.java", 2, ""),
            node(
                "override",
                "method",
                "render",
                "p::Child::render",
                "Child.java",
                3,
                "void (DTO value)",
            ),
            node("other", "class", "Other", "p::Other", "Other.java", 2, ""),
            node(
                "unrelated",
                "method",
                "render",
                "p::Other::render",
                "Other.java",
                3,
                "void (DTO value)",
            ),
            node(
                "caller",
                "class",
                "Caller",
                "p::Caller",
                "Caller.java",
                2,
                "",
            ),
            node(
                "run",
                "method",
                "run",
                "p::Caller::run",
                "Caller.java",
                3,
                "void (Base base, DTO value)",
            ),
        ],
        vec![
            contains("base", "abstract"),
            contains("child", "override"),
            contains("other", "unrelated"),
            contains("caller", "run"),
            GraphEdge {
                kind: "extends".to_owned(),
                ..contains("child", "base")
            },
        ],
    );
    let project = JavaProject::load(root.path(), &graph).unwrap();
    let mut analyzer = SemanticAnalyzer::new(&project);
    let invocation = analyzer
        .method_invocations(&graph.nodes["run"])
        .unwrap()
        .into_iter()
        .find(|site| site.name == "render")
        .unwrap();
    assert!(
        analyzer
            .invocation_reaches(&graph.nodes["run"], &invocation, "override")
            .unwrap()
    );
    assert!(
        !analyzer
            .invocation_reaches(&graph.nodes["run"], &invocation, "unrelated")
            .unwrap()
    );
}

#[test]
fn anonymous_callbacks_resolve_captured_outer_fields() {
    let root = tempfile::tempdir().unwrap();
    let source = "package p;\nclass Service {\nService delegate;\nvoid run() {\nnew Template() { void process() { delegate.run(); } };\n}\n}\n";
    write(root.path(), "Service.java", source);
    let graph = test_snapshot(
        vec![
            node(
                "service",
                "class",
                "Service",
                "p::Service",
                "Service.java",
                2,
                "",
            ),
            node(
                "field",
                "field",
                "delegate",
                "p::Service::delegate",
                "Service.java",
                3,
                "Service delegate",
            ),
            node(
                "run",
                "method",
                "run",
                "p::Service::run",
                "Service.java",
                4,
                "void ()",
            ),
            node(
                "anonymous",
                "class",
                "Anonymous",
                "p::Service::run::<Anonymous>",
                "Service.java",
                5,
                "",
            ),
            node(
                "process",
                "method",
                "process",
                "p::Service::run::<Anonymous>::process",
                "Service.java",
                5,
                "void ()",
            ),
        ],
        vec![
            contains("service", "field"),
            contains("service", "run"),
            contains("anonymous", "process"),
        ],
    );
    let project = JavaProject::load(root.path(), &graph).unwrap();
    let mut analyzer = SemanticAnalyzer::new(&project);
    let invocation = analyzer
        .method_invocations(&graph.nodes["process"])
        .unwrap()
        .into_iter()
        .find(|site| site.name == "run")
        .unwrap();
    assert_eq!(
        analyzer
            .resolve_invocation(&graph.nodes["process"], &invocation)
            .unwrap(),
        ["run"]
    );
}

#[test]
fn nested_types_use_outer_import_without_same_name_fallback() {
    let root = tempfile::tempdir().unwrap();
    for (file, source) in [
        (
            "Caller.java",
            "package p;\nimport correct.Task;\nclass Caller {}",
        ),
        (
            "Missing.java",
            "package p;\nimport missing.Task;\nclass Missing {}",
        ),
        (
            "Correct.java",
            "package correct; class Task { class Result {} }",
        ),
        (
            "Wrong.java",
            "package wrong; class Task { class Result {} }",
        ),
    ] {
        write(root.path(), file, source);
    }
    let graph = test_snapshot(
        vec![
            node(
                "caller",
                "class",
                "Caller",
                "p::Caller",
                "Caller.java",
                3,
                "",
            ),
            node(
                "missing",
                "class",
                "Missing",
                "p::Missing",
                "Missing.java",
                3,
                "",
            ),
            node(
                "correct",
                "class",
                "Result",
                "correct::Task::Result",
                "Correct.java",
                1,
                "",
            ),
            node(
                "wrong",
                "class",
                "Result",
                "wrong::Task::Result",
                "Wrong.java",
                1,
                "",
            ),
        ],
        vec![],
    );
    let project = JavaProject::load(root.path(), &graph).unwrap();
    let kind = parse_java_type("Task.Result").unwrap();
    assert_eq!(
        project
            .resolve_type("Caller.java", "p.Caller", &kind)
            .unwrap()
            .id,
        "correct"
    );
    assert!(
        project
            .resolve_type("Missing.java", "p.Missing", &kind)
            .is_none()
    );
}
