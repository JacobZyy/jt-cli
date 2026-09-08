use super::*;
use tempfile::TempDir;

fn fixture_schema() -> Value {
    json!({"type":"object", "additionalProperties":false,
    "required":["orderId","goods","contact","status","buttons"],
    "properties":{
        "orderId":{"type":"string"},
        "goods":{"type":"object", "description":"商品信息", "additionalProperties":false,
            "properties":{"name":{"type":"string"},"imageUrl":{"type":"string","format":"uri"}}},
        "contact":{"type":"object", "description":"联系人", "properties":{"name":{"type":"string"}}},
        "status":{"type":"integer","enum":[10,20]},
        "buttons":{"type":"array","items":{"type":"integer","enum":[1,2]}},
        "amount":{"type":"number","minimum":100,"maximum":200,"multipleOf":5},
        "date":{"type":"string","format":"date"}
    }})
}

fn operation(schema: Value) -> Value {
    json!({"x-nlab-operation-key":"Facade#detail", "x-nlab-facade":"Facade", "x-nlab-method-name":"detail",
        "responses":{"200":{"content":{"application/json":{"schema":schema}}}}})
}

fn scenario_rules() -> scenarios::Operation {
    serde_json::from_value(
        json!({"coverage":"partial", "base":{"/orderId":"ORDER-123", "/buttons":[]},
        "scenarios":{"pending":{"values":{"/status":10}}, "done":{"values":{"/status":20}}},
        "defaultScenario":"pending", "gaps":["未覆盖取消规则"], "sources":["fixture spec"]}),
    )
    .unwrap()
}

#[test]
fn semantic_generation_uses_context_and_reuses_entity_across_states() {
    let rules = scenarios::Rules::default();
    let operation = operation(fixture_schema());
    let scenarios = scenario_rules();
    let (samples, _) =
        generate_operation(&operation, &json!({}), &rules, &scenarios, 42, "detail").unwrap();
    assert_eq!(samples["pending"]["orderId"], "ORDER-123");
    assert_eq!(samples["pending"]["goods"], samples["done"]["goods"]);
    assert_eq!(samples["pending"]["contact"], samples["done"]["contact"]);
    assert_eq!(samples["pending"]["status"], 10);
    assert_eq!(samples["done"]["status"], 20);
    assert!(
        samples["done"]["goods"]["name"]
            .as_str()
            .unwrap()
            .contains("自行车")
    );
    assert!(
        !samples["done"]["contact"]["name"]
            .as_str()
            .unwrap()
            .contains("自行车")
    );
    assert_eq!(samples["done"]["date"], "2026-09-08");
    assert_eq!(
        samples,
        generate_operation(&operation, &json!({}), &rules, &scenarios, 42, "detail")
            .unwrap()
            .0
    );
}

#[test]
fn invalid_overrides_and_unknown_fields_fail_validation() {
    let rules = scenarios::Rules::default();
    for values in [
        json!({"/status":99}),
        json!({"/missing":1}),
        json!({"/goods":{"invented":1}}),
    ] {
        let scenarios = serde_json::from_value(json!({"base": values})).unwrap();
        assert!(
            generate_operation(
                &operation(fixture_schema()),
                &json!({}),
                &rules,
                &scenarios,
                42,
                "detail"
            )
            .is_err()
        );
    }
}

fn project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".nlab")).unwrap();
    let config = json!({"version":2,"backend":{"repository":"https://example.test/backend.git","branch":"main","appName":"demo","contractRoots":["contract"]},
        "frontend":{"sourceRoot":"src","buildTool":{"kind":"vite","configPath":"vite.config.ts","testConfigs":[]},"tsconfigPath":"tsconfig.json",
            "request":{"module":"@/request","export":"request","responseMode":"unwrapped"},
            "response":{"successCode":"0","codeFields":["code"],"dataFields":["data"],"mockCodeField":"code","mockDataField":"data"},
            "layout":{"preset":"service","implementationDir":"src/service","typesDir":"src/types","enumsDir":"src/enums"},
            "aliases":{"implementation":"@service","types":"@types","enums":"@enums"}}});
    fs::write(
        dir.path().join(".nlab/nlab-api.config.json"),
        config.to_string(),
    )
    .unwrap();
    let mut paths = Map::new();
    for name in ["basic", "partial", "complete", "invalid"] {
        let mut op = operation(fixture_schema());
        op["x-nlab-operation-key"] = json!(name);
        op["x-nlab-method-name"] = json!(name);
        if name == "invalid" {
            op["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["status"]
                ["example"] = json!(999);
        }
        paths.insert(format!("/{name}"), json!({"post":op}));
    }
    fs::write(
        dir.path().join(".nlab/openapi.json"),
        json!({"x-nlab":{"appName":"demo"},"paths":paths}).to_string(),
    )
    .unwrap();
    let mut rules = scenarios::Rules::default();
    rules.operations.insert("partial".into(), scenario_rules());
    let mut complete = scenario_rules();
    complete.coverage = scenarios::Coverage::Complete;
    complete.gaps.clear();
    rules.operations.insert("complete".into(), complete);
    fs::write(
        dir.path().join("rules.json"),
        serde_json::to_string(&rules).unwrap(),
    )
    .unwrap();
    dir
}

fn args(project: &Path) -> MockArgs {
    MockArgs {
        project: project.to_owned(),
        output_root: "mock".into(),
        seed: 42,
        rules: Some("rules.json".into()),
        manifest: None,
        dry_run: false,
        force: false,
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn mixed_tiers_continue_past_failed_operation_and_report_only_written_outputs() {
    let dir = project();
    let result = run_inner(args(dir.path())).unwrap();
    assert_eq!(result["operations"], 3);
    assert_eq!(result["failedOperations"], 1);
    let report = read_json(&dir.path().join("mock/demo/coverage.json"));
    for (name, tier) in [("basic", 1), ("partial", 2), ("complete", 3)] {
        let entry = report["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|op| op["operation"] == name)
            .unwrap();
        assert_eq!(entry["tier"], tier);
        assert_eq!(entry["generation"], "success");
        assert!(
            dir.path()
                .join(entry["files"]["default"].as_str().unwrap())
                .is_file()
        );
    }
    assert!(!dir.path().join("mock/demo/Facade/invalid.json").exists());
    let before = fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap();
    run_inner(args(dir.path())).unwrap();
    assert_eq!(
        before,
        fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap()
    );
}

#[test]
fn modified_files_and_stale_files_are_protected_before_any_write() {
    let dir = project();
    run_inner(args(dir.path())).unwrap();
    let manifest = fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap();
    let file = dir.path().join("mock/demo/Facade/partial.done.json");
    fs::write(&file, "user edit").unwrap();
    let error = run_inner(args(dir.path())).unwrap_err();
    assert!(error.to_string().contains("refuse to overwrite"));
    let mut rules = read_json(&dir.path().join("rules.json"));
    rules["operations"]["partial"]["scenarios"]
        .as_object_mut()
        .unwrap()
        .remove("done");
    fs::write(dir.path().join("rules.json"), rules.to_string()).unwrap();
    assert!(
        run_inner(args(dir.path()))
            .unwrap_err()
            .to_string()
            .contains("refuse to remove")
    );
    assert_eq!(fs::read(file).unwrap(), b"user edit");
    assert_eq!(
        fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap(),
        manifest
    );
}

#[test]
fn dry_run_unknown_operations_and_symlinks_never_write() {
    let dir = project();
    let mut preview = args(dir.path());
    preview.dry_run = true;
    assert_eq!(run_inner(preview).unwrap()["operations"], 0);
    assert!(!dir.path().join("mock").exists());
    let mut rules = read_json(&dir.path().join("rules.json"));
    rules["operations"]["unknown"] = json!({});
    fs::write(dir.path().join("rules.json"), rules.to_string()).unwrap();
    assert!(
        run_inner(args(dir.path()))
            .unwrap_err()
            .to_string()
            .contains("absent from OpenAPI")
    );
    assert!(!dir.path().join("mock").exists());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("mock")).unwrap();
        assert!(safe_target(dir.path(), "mock/demo/a.json").is_err());
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}

#[test]
fn recursive_tree_ends_with_empty_children_without_hiding_other_arrays() {
    let rules = scenarios::Rules::default();
    let node = json!({"type":"object","description":"商品、联系人和可操作按钮", "properties":{
        "goodsTitle":{"type":"string"},
        "goodsImages":{"type":"array","description":"商品主图", "items":{"type":"string"}},
        "children":{"type":"array","items":{"$ref":"#/components/schemas/Node"}},
        "buttonList":{"type":"array","items":{"type":"integer"}}
    },"required":["children","goodsImages","buttonList"]});
    let document = json!({"components":{"schemas":{"Node":node}}});
    let (samples, _) = generate_operation(
        &operation(json!({"$ref":"#/components/schemas/Node"})),
        &document,
        &rules,
        &scenarios::Operation::default(),
        42,
        "tree",
    )
    .unwrap();
    assert_eq!(samples["base"]["children"], json!([]));
    assert_eq!(samples["base"]["buttonList"], json!([]));
    assert_eq!(samples["base"]["goodsImages"].as_array().unwrap().len(), 2);
    assert!(
        samples["base"]["goodsImages"][0]
            .as_str()
            .unwrap()
            .starts_with("https://")
    );
    assert!(
        samples["base"]["goodsTitle"]
            .as_str()
            .unwrap()
            .contains("自行车")
    );
}

#[test]
fn required_nonempty_buttons_need_explicit_rules() {
    let rules = scenarios::Rules::default();
    let schema = json!({"type":"object","properties":{"buttonList":{"type":"array","minItems":1,"items":{"type":"integer","enum":[1]}}}});
    assert!(
        generate_operation(
            &operation(schema.clone()),
            &json!({}),
            &rules,
            &scenarios::Operation::default(),
            42,
            "button"
        )
        .is_err()
    );
    let explicit =
        serde_json::from_value(json!({"base":{"/buttonList":[1]},"sources":["confirmed action"]}))
            .unwrap();
    assert!(
        generate_operation(
            &operation(schema),
            &json!({}),
            &rules,
            &explicit,
            42,
            "button"
        )
        .is_ok()
    );
}

#[test]
fn automatic_stage_consumes_same_rules_and_removes_managed_stale_samples() {
    let dir = project();
    run_inner(args(dir.path())).unwrap();
    let before = fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap();
    let settings = super::super::config::MockSettings {
        enabled: true,
        output_root: "mock".into(),
        seed: 42,
        rules: Some("rules.json".into()),
        manifest: None,
    };
    automatic(dir.path(), &settings).unwrap();
    assert_eq!(
        before,
        fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap()
    );
    let mut rules = read_json(&dir.path().join("rules.json"));
    rules["operations"]["partial"]["scenarios"]
        .as_object_mut()
        .unwrap()
        .remove("done");
    fs::write(dir.path().join("rules.json"), rules.to_string()).unwrap();
    automatic(dir.path(), &settings).unwrap();
    assert!(
        !dir.path()
            .join("mock/demo/Facade/partial.done.json")
            .exists()
    );
    assert!(
        dir.path()
            .join("mock/demo/Facade/partial.pending.json")
            .is_file()
    );
}

#[test]
fn failed_regeneration_removes_old_managed_response_and_does_not_count_it() {
    let dir = project();
    run_inner(args(dir.path())).unwrap();
    let mut openapi = read_json(&dir.path().join(".nlab/openapi.json"));
    openapi["paths"]["/basic"]["post"]["responses"]["200"]["content"]["application/json"]["schema"]
        ["properties"]["status"]["example"] = json!(999);
    fs::write(dir.path().join(".nlab/openapi.json"), openapi.to_string()).unwrap();
    let result = run_inner(args(dir.path())).unwrap();
    assert_eq!(result["operations"], 2);
    assert_eq!(result["failedOperations"], 2);
    assert!(!dir.path().join("mock/demo/Facade/basic.json").exists());
    assert!(!dir.path().join("mock/demo/Facade/basic.base.json").exists());
    let rules = fs::read_to_string(dir.path().join("mock/demo/whistle.rules")).unwrap();
    assert!(
        rules
            .lines()
            .any(|line| line.contains("/basic") && line.contains("statusCode://502"))
    );
    assert!(
        rules
            .lines()
            .filter(|line| !line.starts_with('#'))
            .all(|line| !line.contains('#'))
    );
}

#[test]
fn report_and_rules_edits_are_protected_and_legacy_mock_settings_still_decode() {
    let legacy = serde_json::from_value::<super::super::config::MockSettings>(
        json!({"enabled":true,"outputRoot":"mock","seed":42}),
    )
    .unwrap();
    assert!(legacy.rules.is_none());
    for target in ["mock/demo/coverage.json", "mock/demo/whistle.rules"] {
        let dir = project();
        run_inner(args(dir.path())).unwrap();
        fs::write(dir.path().join(target), "user edit").unwrap();
        assert!(
            run_inner(args(dir.path()))
                .unwrap_err()
                .to_string()
                .contains("refuse to overwrite")
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(target)).unwrap(),
            "user edit"
        );
    }
}

#[test]
fn schema_validation_preserves_nullable_property_and_ignores_unrelated_components() {
    let document =
        json!({"components":{"schemas":{"Unused":{"$ref":"https://example.invalid/schema"}}}});
    let schema = json!({"type":"object", "properties":{"nullable":{"type":"integer"}, "value":{"type":"string","nullable":true}}, "required":["nullable","value"], "additionalProperties":false});
    let validator = schema::validator(&schema, &document).unwrap();
    assert!(validator.is_valid(&json!({"nullable":1,"value":null})));
    assert!(!validator.is_valid(&json!({"nullable":"not a number","value":null})));
}

#[test]
fn independent_manifest_preserves_old_root_and_manual_changes() {
    let dir = project();
    run_inner(args(dir.path())).unwrap();
    let legacy = fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap();
    fs::write(
        dir.path().join("mock/demo/Facade/basic.json"),
        "user edited response",
    )
    .unwrap();
    fs::write(
        dir.path().join("mock/demo/whistle.rules"),
        "user edited rules",
    )
    .unwrap();
    let mut isolated = args(dir.path());
    isolated.output_root = "independent".into();
    assert!(
        run_inner(isolated.clone())
            .unwrap_err()
            .to_string()
            .contains("belongs to another output root")
    );
    assert!(!dir.path().join("independent").exists());
    isolated.manifest = Some(".nlab/independent-manifest.json".into());
    assert_eq!(run_inner(isolated.clone()).unwrap()["operations"], 3);
    assert_eq!(run_inner(isolated).unwrap()["operations"], 3);
    assert_eq!(
        fs::read(dir.path().join(".nlab/mock-manifest.json")).unwrap(),
        legacy
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("mock/demo/Facade/basic.json")).unwrap(),
        "user edited response"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("mock/demo/whistle.rules")).unwrap(),
        "user edited rules"
    );
    assert!(dir.path().join(".nlab/independent-manifest.json").is_file());
    assert!(
        dir.path()
            .join("independent/demo/Facade/basic.json")
            .is_file()
    );
}
