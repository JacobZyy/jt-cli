use super::*;
use crate::graph::test_snapshot;
use crate::java::JavaProject;
use crate::semantic::tests::{node, write};

fn domain(source: &str, accessor: &str) -> Domain {
    named_domain(source, "StateEnum", accessor)
}

fn named_domain(source: &str, enum_name: &str, accessor: &str) -> Domain {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "State.java", source);
    let graph = test_snapshot(
        vec![node(
            "state",
            "enum",
            enum_name,
            &format!("p::{enum_name}"),
            "State.java",
            2,
            "",
        )],
        vec![],
    );
    let project = JavaProject::load(root.path(), &graph).unwrap();
    extract_enum_domain(&project, &graph.nodes["state"], accessor).unwrap()
}

#[test]
fn only_primary_value_getter_produces_enum_domain() {
    let source = r#"package p;
enum StateEnum {
    READY(1, "Ready", "blue", 10), DONE(2, "Done", "green", 20);
    final int state;
    final String name;
    final String color;
    final int buttonType;
    StateEnum(int state, String name, String color, int buttonType) {
        this.state = state;
        this.name = name;
        this.color = color;
        this.buttonType = buttonType;
    }
    int getState() { return state; }
    String getName() { return name; }
    String getColor() { return color; }
    int getButtonType() { return buttonType; }
}
"#;

    let state = domain(source, "getState");
    assert!(state.primary_enum_value);
    assert!(state.complete);
    assert_eq!(state.values.len(), 2);

    for accessor in ["getName", "getColor", "getButtonType"] {
        let derived = domain(source, accessor);
        assert!(!derived.primary_enum_value, "{accessor}");
        assert!(derived.values.is_empty(), "{accessor}");
    }
}

#[test]
fn name_lookup_never_becomes_primary_value() {
    let source = r#"package p;
enum StateEnum {
    READY(1, "Ready"), DONE(2, "Done");
    final int state;
    final String name;
    StateEnum(int state, String name) { this.state = state; this.name = name; }
    int getState() { return state; }
    String getName() { return name; }
    static StateEnum fromName(String name) {
        for (StateEnum item : values()) { if (item.name.equals(name)) return item; }
        return null;
    }
}
"#;
    let derived = domain(source, "getName");
    assert!(!derived.primary_enum_value);
    assert!(derived.values.is_empty());
}

#[test]
fn val_and_json_value_can_identify_primary_field() {
    let val_source = r#"package p;
enum StateEnum {
    READY(1), DONE(2);
    final int state;
    StateEnum(int state) { this.state = state; }
    int val() { return state; }
}
"#;
    let val = domain(val_source, "val");
    assert!(val.primary_enum_value && val.complete);
    assert_eq!(
        val.values
            .iter()
            .map(|value| value.value.clone())
            .collect::<Vec<_>>(),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );

    let json_value_source = r#"package p;
enum StateEnum {
    READY(1), DONE(2);
    @JsonValue final int state;
    StateEnum(int state) { this.state = state; }
}
"#;
    let json_value = domain(json_value_source, "state");
    assert!(json_value.primary_enum_value && json_value.complete);
}

#[test]
fn constructor_parameter_order_does_not_change_values() {
    let source = r#"package p;
enum StateEnum {
    READY(1, "Ready"), DONE(2, "Done");
    final String name;
    final int state;
    StateEnum(int state, String name) {
        this.state = state;
        this.name = name;
    }
    int getState() { return state; }
}
"#;
    let extracted = domain(source, "getState");
    assert!(extracted.primary_enum_value && extracted.complete);
    assert_eq!(
        extracted
            .values
            .iter()
            .map(|value| value.value.clone())
            .collect::<Vec<_>>(),
        vec![WireValue::Number(1), WireValue::Number(2)]
    );
}

#[test]
fn ambiguous_duplicate_and_transformed_primary_values_stay_unconfirmed() {
    let ambiguous = r#"package p;
enum StateEnum {
    READY(1, 1), DONE(2, 2);
    final int state;
    final int code;
    StateEnum(int state, int code) { this.state = state; this.code = code; }
    int getState() { return state; }
}
"#;
    let ambiguous = domain(ambiguous, "getState");
    assert!(!ambiguous.primary_enum_value);

    let duplicate = r#"package p;
enum StateEnum {
    READY(1), DONE(1);
    final int state;
    StateEnum(int state) { this.state = state; }
    int getState() { return state; }
}
"#;
    let duplicate = domain(duplicate, "getState");
    assert!(duplicate.primary_enum_value);
    assert!(!duplicate.complete);

    let transformed = r#"package p;
enum StateEnum {
    READY(1), DONE(2);
    final int state;
    StateEnum(int state) { this.state = state; }
    int getState() { return normalize(state); }
    int normalize(int value) { return value; }
}
"#;
    let transformed = domain(transformed, "getState");
    assert!(!transformed.primary_enum_value);
    assert!(transformed.values.is_empty());
}

#[test]
fn constructor_rewrites_and_calls_are_not_direct_value_bindings() {
    for body in [
        "this.state = state; this.state++;",
        "state = 9; this.state = state;",
        "this.state = state; normalize();",
        "this.state = state; this.label = normalize();",
    ] {
        let source = format!(
            "package p;\nenum StateEnum {{ A(1), B(2); int state; String label; StateEnum(int state) {{ {body} }} int getState() {{ return state; }} String normalize() {{ this.state=99; return \"x\"; }} }}"
        );
        assert!(!domain(&source, "getState").complete, "{body}");
    }
    let source = "package p;\nenum StateEnum { A(1,2), B(3,4); @JsonValue(false) final int state; final int code; StateEnum(int state, int code) { this.state=state; this.code=code; } int getState() { return state; } }";
    assert!(!domain(source, "getState").primary_enum_value);
}

#[test]
fn naming_conventions_do_not_turn_display_attributes_into_primary_values() {
    for (enum_name, field, accessor) in [
        ("ColorEnum", "color", "getColor"),
        ("NameEnum", "name", "getName"),
        ("ButtonTypeEnum", "buttonType", "getButtonType"),
    ] {
        let source = format!(
            "package p;\nenum {enum_name} {{ A(1), B(2); final int {field}; {enum_name}(int value) {{ this.{field}=value; }} int {accessor}() {{ return {field}; }} }}"
        );
        assert!(!named_domain(&source, enum_name, accessor).primary_enum_value);
    }
    let source = "package p;\nenum ActionEnum { A(\"a\", 1), B(\"b\", 2); final String actionCode; final int buttonType; ActionEnum(String actionCode, int buttonType) { this.actionCode=actionCode; this.buttonType=buttonType; } String getActionCode() { return actionCode; } int getButtonType() { return buttonType; } }";
    assert!(named_domain(source, "ActionEnum", "getActionCode").complete);
    assert!(!named_domain(source, "ActionEnum", "getButtonType").primary_enum_value);
}
