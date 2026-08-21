use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::model::{CodedValue, CodedValueSource, CodedValues, TypeRef, WireValue};

static EXPLICIT_PAIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)(?:^|[\s,，;；])(?P<code>[A-Za-z][A-Za-z0-9_-]*|-?\d+(?:_\d+)?)\s*[-:：]\s*(?P<label>[^,，;；\n]+)",
    )
    .expect("coded value pair regex")
});
static NUMBERED_STRING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*(?:[-*+]\s*)?(?:\d+[.)、]|\(\d+\))\s*(?P<code>[A-Za-z][A-Za-z0-9_-]*)\s+(?P<label>.+)$",
    )
    .expect("numbered string coded value regex")
});
static NUMBERED_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^\s*(?:[-*+]\s*)?(?:\(?(?P<code>-?\d+)\)?[.)、]?|(?P<plain>-?\d+))\s+(?P<label>.+)$",
    )
    .expect("numbered numeric coded value regex")
});
static GROUPED_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?x)^\s*(?P<codes>-?\d+(?:\s*/\s*-?\d+)+)\s*[-:：]\s*(?P<label>.+)$")
        .expect("grouped numeric coded value regex")
});

pub fn parse(
    field_name: &str,
    description: Option<&str>,
    java_type: &TypeRef,
) -> Option<CodedValues> {
    let description = description?.trim();
    if description.is_empty() || looks_like_date_format(description) {
        return None;
    }
    let numeric = is_numeric(java_type);
    let semantic_numeric = numeric
        && ["类型", "状态", "取值", "编码", "按钮类型"]
            .iter()
            .any(|marker| description.contains(marker));
    let mut values = BTreeMap::<String, CodedValue>::new();
    for captures in EXPLICIT_PAIR.captures_iter(description) {
        let code = captures.name("code")?.as_str();
        let label = clean_label(captures.name("label")?.as_str());
        if code
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_alphabetic())
            && label
                .chars()
                .next()
                .is_some_and(|value| value.is_ascii_alphanumeric())
        {
            continue;
        }
        if let Some(value) = wire_value(code, numeric) {
            values.entry(wire_key(&value)).or_insert(CodedValue {
                value,
                key: None,
                label,
            });
        }
    }
    for line in description.lines() {
        if let Some(captures) = NUMBERED_STRING.captures(line) {
            let code = captures.name("code")?.as_str();
            let value = WireValue::String(code.to_owned());
            values.entry(wire_key(&value)).or_insert(CodedValue {
                value,
                key: None,
                label: clean_label(captures.name("label")?.as_str()),
            });
            continue;
        }
        if semantic_numeric {
            if let Some(captures) = GROUPED_NUMBER.captures(line) {
                let label = clean_label(captures.name("label")?.as_str());
                for code in captures.name("codes")?.as_str().split('/') {
                    if let Some(value) = wire_value(code.trim(), true) {
                        values.entry(wire_key(&value)).or_insert(CodedValue {
                            value,
                            key: None,
                            label: label.clone(),
                        });
                    }
                }
                continue;
            }
            if let Some(captures) = NUMBERED_NUMBER.captures(line) {
                let code = captures
                    .name("code")
                    .or_else(|| captures.name("plain"))?
                    .as_str();
                if let Some(value) = wire_value(code, true) {
                    values.entry(wire_key(&value)).or_insert(CodedValue {
                        value,
                        key: None,
                        label: clean_label(captures.name("label")?.as_str()),
                    });
                }
            }
        }
    }
    (values.len() >= 2).then(|| CodedValues {
        name: format!("{}Values", upper_camel(field_name)),
        source: CodedValueSource::Comment,
        values: values.into_values().collect(),
    })
}

fn wire_value(value: &str, numeric: bool) -> Option<WireValue> {
    if numeric {
        value
            .replace('_', "")
            .parse::<i64>()
            .ok()
            .map(WireValue::Number)
    } else {
        Some(WireValue::String(value.to_owned()))
    }
}

fn is_numeric(java_type: &TypeRef) -> bool {
    matches!(
        java_type.simple_name(),
        "Integer"
            | "int"
            | "Short"
            | "short"
            | "Byte"
            | "byte"
            | "Long"
            | "long"
            | "BigInteger"
            | "Double"
            | "double"
            | "Float"
            | "float"
            | "BigDecimal"
    )
}

fn looks_like_date_format(value: &str) -> bool {
    ["yyyy-MM-dd", "HH:mm:ss", "yyyy/MM/dd"]
        .iter()
        .any(|pattern| value.contains(pattern))
}

fn clean_label(value: &str) -> String {
    value.trim().trim_end_matches(['.', '。']).trim().to_owned()
}

fn wire_key(value: &WireValue) -> String {
    match value {
        WireValue::String(value) => format!("s:{value}"),
        WireValue::Number(value) => format!("n:{value}"),
    }
}

fn upper_camel(value: &str) -> String {
    let mut output = String::new();
    let mut uppercase = true;
    for character in value.chars() {
        if !character.is_ascii_alphanumeric() {
            uppercase = true;
        } else if uppercase {
            output.extend(character.to_uppercase());
            uppercase = false;
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn java_type(name: &str) -> TypeRef {
        TypeRef {
            name: name.to_owned(),
            arguments: vec![],
            array_depth: 0,
        }
    }

    #[test]
    fn parses_numbered_string_codes_without_using_ordinals() {
        let values = parse(
            "scene",
            Some("按钮场景\n1. recycle-list 回收单列表\n2. refurb-list 整备单列表"),
            &java_type("String"),
        )
        .unwrap();
        assert_eq!(
            values
                .values
                .iter()
                .map(|value| &value.value)
                .collect::<Vec<_>>(),
            vec![
                &WireValue::String("recycle-list".to_owned()),
                &WireValue::String("refurb-list".to_owned())
            ]
        );
    }

    #[test]
    fn numeric_ordinals_require_semantic_header() {
        assert!(
            parse(
                "step",
                Some("操作步骤\n1. 打开页面\n2. 提交表单"),
                &java_type("Integer")
            )
            .is_none()
        );
        assert_eq!(
            parse(
                "buttonType",
                Some("按钮类型：\n1. 红色实心\n2. 红色空心\n3. 灰色空心"),
                &java_type("Integer")
            )
            .unwrap()
            .values
            .len(),
            3
        );
    }
}
