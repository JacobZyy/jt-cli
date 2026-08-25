use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;

use super::model::{CodedValue, CodedValueSource, CodedValues, TypeRef, WireValue};

static DATE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:(?:yyyy[-/]MM[-/]dd|HH:mm(?::ss)?)|\d{4}[-/]\d{1,2}[-/]\d{1,2}|\d{1,2}-\d{1,2}\s+\d{1,2}:\d{2}).*$",
    )
    .expect("date line regex")
});
static DATE_FORMAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:yyyy[-/]MM[-/]dd|HH:mm(?::ss)?)").expect("date format regex"));
static CALENDAR_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{4}[-/]\d{1,2}[-/]\d{1,2}\b").expect("calendar date regex"));
static MONTH_DAY_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{1,2}-\d{1,2}\s+\d{1,2}:\d{2}\b").expect("month day time regex")
});
static CLOCK_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{1,2}:\d{2}(?::\d{2})?\b").expect("clock time regex"));
static NUMERIC_MAPPING_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)(?:^|[\s,，;；、:：（(])(?P<code>-?\d+(?:\.\d+)?(?:_\d+)*(?:\s*[/／]\s*-?\d+(?:\.\d+)?)*)\s*[-=：:]\s*",
    )
    .expect("numeric mapping start regex")
});
static STRING_MAPPING_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)(?:^|[\s,，;；、:：（(])(?:
            (?P<colon>-?\d+(?:\.\d+)?(?:_\d+)*(?:\s*[/／]\s*-?\d+(?:\.\d+)?)*|[A-Za-z][A-Za-z0-9_$.-]*)\s*[=：:] |
            (?P<hyphen>-?\d+(?:\.\d+)?(?:_\d+)*(?:\s*[/／]\s*-?\d+(?:\.\d+)?)*|[A-Za-z][A-Za-z0-9_$.]*)\s*-
        )\s*",
    )
    .expect("string mapping start regex")
});
static NUMBER_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:[^：:\n]{0,32})?(?:类型|状态码?|取值|可选值|枚举值?|编码|代码|\bcode\b|\benum(?:\s+values?)?\b)\s*[：:]\s*$",
    )
    .expect("number header regex")
});
static INLINE_NUMBER_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:[^：:\n]{0,32})?(?:类型|状态码?|取值|可选值|枚举值?|编码|代码|\bcode\b|\benum(?:\s+values?)?\b)\s*[：:]\s*",
    )
    .expect("inline number header regex")
});
static NUMBER_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^(?:[-*+]\s+)?(?:
            [（(]\s*(?P<wrapped>-?\d+(?:\s*[/／]\s*-?\d+)*)\s*[）)] |
            (?P<plain>-?\d+(?:\s*[/／]\s*-?\d+)*)(?:\s*[、)）:：=-]\s*|[.．]\s+|\s+)
        )\s*(?P<label>\S.*?)\s*$",
    )
    .expect("number item regex")
});
static INLINE_FIRST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[。！？]\s*)(?P<code>-?\d+(?:\s*[/／]\s*-?\d+)*)\s+")
        .expect("inline first item regex")
});
static INLINE_DELIMITED_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[、,，;；]\s*)(?P<code>-?\d+(?:\s*[/／]\s*-?\d+)*)\s+")
        .expect("inline delimited item regex")
});
static INLINE_SPACE_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\s+)(?P<code>-?\d+(?:\s*[/／]\s*-?\d+)*)\s+")
        .expect("inline space item regex")
});
static TRAILING_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[;；]\s*(?:19|20)\d{2}(?:\s|$)").expect("trailing year regex"));
static NUMBER_AT_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-?\d+(?:\s*[/／]\s*-?\d+)*\s+").expect("number at start regex"));
static STRING_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:[^：:\n]{0,32})?(?:类型|状态|取值|可选值|枚举值?|编码|代码|标识|场景|\bcode\b|\benum(?:\s+values?)?\b)\s*[：:]?\s*$",
    )
    .expect("string header regex")
});
static EXPLICIT_STRING_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:取值|可选值|枚举值?|编码|代码|标识|\bcode\b|\benum\b)")
        .expect("explicit string header regex")
});
static ORDERED_STRING_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^(?:[-*+]\s+)?(?:
            [（(]\s*(?P<wrapped>\d+)\s*[）)] |
            (?P<plain>\d+)(?:\s*[、)）:：=-]\s*|[.．]\s+|\s+)
        )\s*(?P<code>[A-Za-z_$][A-Za-z0-9_.$-]*)\s+(?P<label>\S.*?)\s*$",
    )
    .expect("ordered string item regex")
});
static UNORDERED_STRING_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[-*+]\s+(?P<code>[A-Za-z_$][A-Za-z0-9_.$-]*)\s+(?P<label>\S.*?)\s*$")
        .expect("unordered string item regex")
});
static SPECIAL_STRING_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[-_.$]|\d|^[A-Z][A-Z0-9_]*$").expect("special string code regex")
});
static YEAR_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:19|20)\d{2}\s*年").expect("year line regex"));
static NOTE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:备注|说明|注)\s*[：:]").expect("note line regex"));
static CONSTANT_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[（(](?P<key>[A-Z][A-Z0-9_]*)[）)]\s*$").expect("constant key regex")
});

#[derive(Clone, Copy, Eq, PartialEq)]
enum ValueKind {
    Number,
    String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ListStyle {
    Ordered,
    Unordered,
}

pub fn parse(
    field_name: &str,
    comment: Option<&str>,
    annotations: Option<&str>,
    java_type: &TypeRef,
) -> Option<CodedValues> {
    let kind = value_kind(java_type)?;
    for (source, text) in [
        (CodedValueSource::Comment, comment),
        (CodedValueSource::Annotation, annotations),
    ] {
        let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
            continue;
        };
        let values = parse_pairs(text, kind);
        if values.len() < 2 {
            continue;
        }
        return Some(CodedValues {
            name: format!("{}Values", upper_camel(field_name)),
            source: if values.iter().all(|value| value.key.is_some()) {
                CodedValueSource::ConstantReference
            } else {
                source
            },
            values,
        });
    }
    None
}

pub fn with_fallback_keys(field_name: &str, values: &[CodedValue]) -> Vec<CodedValue> {
    values
        .iter()
        .cloned()
        .map(|mut value| {
            if value.key.is_none() {
                value.key = Some(fallback_key(field_name, &value.value));
            }
            value
        })
        .collect()
}

fn parse_pairs(text: &str, kind: ValueKind) -> Vec<CodedValue> {
    let source = sanitize(text);
    let structured = match kind {
        ValueKind::Number => parse_structured_numbers(&source),
        ValueKind::String => parse_string_list(&source),
    };
    if let Some(values) = structured {
        return values;
    }
    let values = parse_mappings(&source, kind);
    if values.len() >= 2 || kind == ValueKind::String {
        return values;
    }
    parse_inline_numbers(&source)
}

fn sanitize(text: &str) -> String {
    let value = DATE_LINE.replace_all(text, " ");
    let value = DATE_FORMAT.replace_all(&value, " ");
    let value = CALENDAR_DATE.replace_all(&value, " ");
    let value = MONTH_DAY_TIME.replace_all(&value, " ");
    CLOCK_TIME.replace_all(&value, " ").into_owned()
}

fn parse_mappings(source: &str, kind: ValueKind) -> Vec<CodedValue> {
    let starts = match kind {
        ValueKind::Number => mapping_starts(&NUMERIC_MAPPING_START, source, "code", None),
        ValueKind::String => mapping_starts(&STRING_MAPPING_START, source, "colon", Some("hyphen")),
    };
    let mut values = Vec::new();
    for (index, (raw_values, _, label_start)) in starts.iter().enumerate() {
        let label_end = starts
            .get(index + 1)
            .map(|(_, start, _)| *start)
            .unwrap_or(source.len());
        append_values(
            &mut values,
            raw_values,
            mapping_label(&source[*label_start..label_end]),
            kind,
        );
    }
    values
}

fn mapping_starts<'a>(
    pattern: &Regex,
    source: &'a str,
    first: &str,
    second: Option<&str>,
) -> Vec<(&'a str, usize, usize)> {
    pattern
        .captures_iter(source)
        .filter_map(|captures| {
            let matched = captures.get(0)?;
            let code = captures
                .name(first)
                .or_else(|| second.and_then(|name| captures.name(name)))?;
            Some((code.as_str(), matched.start(), matched.end()))
        })
        .collect()
}

fn mapping_label(value: &str) -> &str {
    for (index, character) in value.char_indices() {
        if matches!(character, '。' | '！' | '？') {
            return &value[..index];
        }
        if matches!(character, '.' | '!' | '?') {
            let tail = &value[index + character.len_utf8()..];
            if tail.is_empty() || tail.chars().next().is_some_and(char::is_whitespace) {
                return &value[..index];
            }
        }
    }
    value
}

fn parse_structured_numbers(text: &str) -> Option<Vec<CodedValue>> {
    let lines = nonempty_lines(text);
    if lines.len() < 3 || !NUMBER_HEADER.is_match(lines[0]) {
        return None;
    }
    let mut values = Vec::new();
    for line in &lines[1..] {
        if values.len() >= 2 && YEAR_LINE.is_match(line) {
            break;
        }
        let Some(captures) = NUMBER_ITEM.captures(line) else {
            if values.len() >= 2 && NOTE_LINE.is_match(line) {
                break;
            }
            return Some(Vec::new());
        };
        let Some(code) = captures.name("wrapped").or_else(|| captures.name("plain")) else {
            return Some(Vec::new());
        };
        let Some(label) = captures.name("label") else {
            return Some(Vec::new());
        };
        append_values(
            &mut values,
            code.as_str(),
            label.as_str(),
            ValueKind::Number,
        );
    }
    Some(values)
}

fn parse_inline_numbers(text: &str) -> Vec<CodedValue> {
    let Some(header) = INLINE_NUMBER_HEADER.find(text) else {
        return Vec::new();
    };
    let body = &text[header.end()..];
    let Some(first) = INLINE_FIRST_ITEM.captures(body) else {
        return Vec::new();
    };
    let Some(first_code) = first.name("code") else {
        return Vec::new();
    };
    let mut list = body[first_code.start()..]
        .split(['。', '！', '？'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if let Some(year) = TRAILING_YEAR.find(&list) {
        list.truncate(year.start());
    } else if let Some(note) = trailing_note(&list) {
        list.truncate(note);
    }
    let delimited = item_starts(&INLINE_DELIMITED_ITEM, &list);
    let mut values = values_from_starts(&list, &delimited, ValueKind::Number);
    if values.len() >= 2 {
        return values;
    }
    let spaced = item_starts(&INLINE_SPACE_ITEM, &list);
    let codes = spaced
        .iter()
        .map(|(code, _, _)| code.parse::<i64>())
        .collect::<Result<Vec<_>, _>>();
    let Ok(codes) = codes else {
        return Vec::new();
    };
    if codes.len() < 2
        || codes
            .windows(2)
            .any(|pair| pair[1] != pair[0].saturating_add(1))
    {
        return Vec::new();
    }
    values = values_from_starts(&list, &spaced, ValueKind::Number);
    values
}

fn trailing_note(value: &str) -> Option<usize> {
    value.char_indices().find_map(|(index, character)| {
        if !matches!(character, ';' | '；') {
            return None;
        }
        let tail = value[index + character.len_utf8()..].trim_start();
        (!NUMBER_AT_START.is_match(tail)).then_some(index)
    })
}

fn item_starts<'a>(pattern: &Regex, value: &'a str) -> Vec<(&'a str, usize, usize)> {
    pattern
        .captures_iter(value)
        .filter_map(|captures| {
            let matched = captures.get(0)?;
            Some((
                captures.name("code")?.as_str(),
                matched.start(),
                matched.end(),
            ))
        })
        .collect()
}

fn values_from_starts(
    source: &str,
    starts: &[(&str, usize, usize)],
    kind: ValueKind,
) -> Vec<CodedValue> {
    let mut values = Vec::new();
    for (index, (code, _, label_start)) in starts.iter().enumerate() {
        let label_end = starts
            .get(index + 1)
            .map(|(_, start, _)| *start)
            .unwrap_or(source.len());
        append_values(&mut values, code, &source[*label_start..label_end], kind);
    }
    values
}

fn parse_string_list(text: &str) -> Option<Vec<CodedValue>> {
    let lines = nonempty_lines(text);
    if lines.len() < 3 || !STRING_HEADER.is_match(lines[0]) {
        return None;
    }
    let explicit_code_header = EXPLICIT_STRING_HEADER.is_match(lines[0]);
    let mut values = Vec::new();
    let mut ordinals = Vec::new();
    let mut style = None;
    for line in &lines[1..] {
        if values.len() >= 2 && YEAR_LINE.is_match(line) {
            break;
        }
        let ordered = ORDERED_STRING_ITEM.captures(line);
        let unordered = ordered
            .is_none()
            .then(|| UNORDERED_STRING_ITEM.captures(line))
            .flatten();
        let current_style = if ordered.is_some() {
            ListStyle::Ordered
        } else if unordered.is_some() {
            ListStyle::Unordered
        } else {
            if values.len() >= 2 && NOTE_LINE.is_match(line) {
                break;
            }
            return Some(Vec::new());
        };
        if style.is_some_and(|existing| existing != current_style) {
            return Some(Vec::new());
        }
        style = Some(current_style);
        let captures = ordered.as_ref().or(unordered.as_ref())?;
        let code = captures.name("code")?.as_str();
        if !explicit_code_header && !SPECIAL_STRING_CODE.is_match(code) {
            return Some(Vec::new());
        }
        if current_style == ListStyle::Ordered {
            let ordinal = captures
                .name("wrapped")
                .or_else(|| captures.name("plain"))?
                .as_str()
                .parse::<i64>()
                .ok()?;
            ordinals.push(ordinal);
        }
        append_values(
            &mut values,
            code,
            captures.name("label")?.as_str(),
            ValueKind::String,
        );
    }
    if style == Some(ListStyle::Ordered)
        && ordinals
            .windows(2)
            .any(|pair| pair[1] != pair[0].saturating_add(1))
    {
        return Some(Vec::new());
    }
    Some(values)
}

fn nonempty_lines(value: &str) -> Vec<&str> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn append_values(values: &mut Vec<CodedValue>, raw_values: &str, raw_label: &str, kind: ValueKind) {
    let mut label = raw_label
        .trim()
        .split(['。', '；', ';'])
        .next()
        .unwrap_or_default()
        .trim_end_matches([',', '，'])
        .trim()
        .to_owned();
    if label.is_empty() {
        return;
    }
    let constant_key = CONSTANT_KEY.captures(&label).and_then(|captures| {
        Some((
            captures.get(0)?.start(),
            captures.name("key")?.as_str().to_owned(),
        ))
    });
    let key = constant_key.map(|(start, key)| {
        label.truncate(start);
        label = label.trim().to_owned();
        key
    });
    for raw_value in raw_values.split(['/', '／']).map(str::trim) {
        let value = match kind {
            ValueKind::Number => parse_number(raw_value),
            ValueKind::String => Some(WireValue::String(raw_value.to_owned())),
        };
        let Some(value) = value else {
            continue;
        };
        if values.iter().any(|item| item.value == value) {
            continue;
        }
        values.push(CodedValue {
            value,
            label: label.clone(),
            key: key.clone(),
        });
    }
}

fn parse_number(value: &str) -> Option<WireValue> {
    if value.contains('.') {
        serde_json::Number::from_str(value)
            .ok()
            .map(WireValue::Decimal)
    } else {
        value.parse::<i64>().ok().map(WireValue::Number)
    }
}

fn value_kind(java_type: &TypeRef) -> Option<ValueKind> {
    match java_type.simple_name() {
        "Integer" | "int" | "Short" | "short" | "Byte" | "byte" | "Double" | "double" | "Float"
        | "float" | "BigDecimal" => Some(ValueKind::Number),
        "String" | "CharSequence" | "char" | "Character" | "Long" | "long" | "BigInteger"
        | "Date" | "LocalDate" | "LocalDateTime" | "Instant" | "Timestamp" => {
            Some(ValueKind::String)
        }
        _ => None,
    }
}

fn fallback_key(field_name: &str, value: &WireValue) -> String {
    let raw_value = match value {
        WireValue::String(value) => value.clone(),
        WireValue::Number(value) => value.to_string(),
        WireValue::Decimal(value) => value.to_string(),
    };
    let raw_value = raw_value
        .strip_prefix('-')
        .map(|value| format!("NEGATIVE_{value}"))
        .unwrap_or(raw_value);
    let value = upper_snake(&raw_value);
    format!(
        "{}_{}",
        upper_snake(field_name),
        if value.is_empty() { "VALUE" } else { &value }
    )
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

fn upper_snake(value: &str) -> String {
    let mut output = String::new();
    let mut previous_is_lower_or_digit = false;
    for character in value.chars() {
        if !character.is_ascii_alphanumeric() {
            if !output.is_empty() && !output.ends_with('_') {
                output.push('_');
            }
            previous_is_lower_or_digit = false;
        } else {
            if character.is_ascii_uppercase()
                && previous_is_lower_or_digit
                && !output.ends_with('_')
            {
                output.push('_');
            }
            output.push(character.to_ascii_uppercase());
            previous_is_lower_or_digit =
                character.is_ascii_lowercase() || character.is_ascii_digit();
        }
    }
    output.trim_matches('_').to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn java_type(name: &str) -> TypeRef {
        TypeRef {
            name: name.to_owned(),
            arguments: vec![],
            array_depth: 0,
        }
    }

    fn assert_values(field: &str, java: &str, comment: &str, expected: Vec<(Value, &'static str)>) {
        let values = parse(field, Some(comment), None, &java_type(java)).unwrap();
        let actual = values
            .values
            .iter()
            .map(|value| {
                (
                    serde_json::to_value(&value.value).unwrap(),
                    value.label.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "field {field}");
    }

    #[test]
    fn old_skill_comment_matrix_is_equivalent() {
        assert_values(
            "scene",
            "String",
            "按钮场景\n1. recycle-list 回收单列表\n2. refurb-list 整备单列表",
            vec![
                (json!("recycle-list"), "回收单列表"),
                (json!("refurb-list"), "整备单列表"),
            ],
        );
        assert_values(
            "buttonType",
            "Integer",
            "按钮类型：\n1. 红色实心\n2. 红色空心\n3. 灰色空心",
            vec![
                (json!(1), "红色实心"),
                (json!(2), "红色空心"),
                (json!(3), "灰色空心"),
            ],
        );
        assert_values(
            "extendedType",
            "Integer",
            "扩展类型：\n1、顿号\n2)半角括号\n（3）全角括号\n- 4: Markdown 减号\n* 5: Markdown 星号\n+ 6=Markdown 加号",
            vec![
                (json!(1), "顿号"),
                (json!(2), "半角括号"),
                (json!(3), "全角括号"),
                (json!(4), "Markdown 减号"),
                (json!(5), "Markdown 星号"),
                (json!(6), "Markdown 加号"),
            ],
        );
        assert_values(
            "searchType",
            "Integer",
            "搜索类型：可选。1 整车商品 2 零件配件商品 3 服务商品；不传时不限定业务线",
            vec![
                (json!(1), "整车商品"),
                (json!(2), "零件配件商品"),
                (json!(3), "服务商品"),
            ],
        );
        assert_values(
            "state",
            "Integer",
            "交易状态码：1 待支付、3 待核销、4/5/6 已核销",
            vec![
                (json!(1), "待支付"),
                (json!(3), "待核销"),
                (json!(4), "已核销"),
                (json!(5), "已核销"),
                (json!(6), "已核销"),
            ],
        );
        assert_values(
            "groupedMarkdownState",
            "Integer",
            "状态码：\n- 1:待支付\n- 4/5/6:已核销",
            vec![
                (json!(1), "待支付"),
                (json!(4), "已核销"),
                (json!(5), "已核销"),
                (json!(6), "已核销"),
            ],
        );
        for (field, comment) in [
            (
                "notedStatus",
                "状态：\n1、待处理\n2、已完成\n备注：后续可增加状态",
            ),
            (
                "structuredYearStatus",
                "状态：\n1、待处理\n2、已完成\n2026 年新增",
            ),
            (
                "structuredDateStatus",
                "状态：\n1、待处理\n2、已完成\n2026-08-18 新增",
            ),
        ] {
            assert_values(
                field,
                "Integer",
                comment,
                vec![(json!(1), "待处理"), (json!(2), "已完成")],
            );
        }
        assert_values(
            "yearNoteType",
            "Integer",
            "类型：1 旧版、2 新版；2026 年新增说明",
            vec![(json!(1), "旧版"), (json!(2), "新版")],
        );
        assert_values(
            "datedNoteStatus",
            "Integer",
            "状态：0-否，1-是；2026-08-18 新增",
            vec![(json!(0), "否"), (json!(1), "是")],
        );
        assert_values(
            "markdownAction",
            "String",
            "操作标识：\n- start_qc 开始质检\n* cancel 取消",
            vec![(json!("start_qc"), "开始质检"), (json!("cancel"), "取消")],
        );
        assert_values(
            "inlineScene",
            "String",
            "按钮场景：recycle-list:回收，refurb-list:整备",
            vec![
                (json!("recycle-list"), "回收"),
                (json!("refurb-list"), "整备"),
            ],
        );

        for (field, java, comment) in [
            (
                "retryCount",
                "Integer",
                "重试流程：\n1. 建立连接\n2. 发送请求",
            ),
            ("dateValue", "Integer", "日期取值：\n2025-08-18\n2026-08-18"),
            ("ratio", "Double", "倍率取值：\n1.5 倍率\n2.0 倍率"),
            ("unorderedStatus", "Integer", "状态：\n- 待处理\n- 已完成"),
            ("createdAt", "String", "创建时间起 yyyy-MM-dd HH:mm:ss"),
            (
                "proseList",
                "String",
                "返回类型：\n- 支持批量查询\n- 不支持匿名访问",
            ),
            (
                "englishProseList",
                "String",
                "返回类型：\n1. support batch query\n2. reject anonymous access",
            ),
        ] {
            assert!(
                parse(field, Some(comment), None, &java_type(java)).is_none(),
                "field {field} was mistaken for coded values"
            );
        }
    }

    #[test]
    fn supports_annotations_constant_keys_decimals_and_long_strings() {
        let annotation = parse(
            "status",
            None,
            Some("@Schema(description = \"状态：1-待处理 2-已完成\")"),
            &java_type("Integer"),
        )
        .unwrap();
        assert_eq!(annotation.source, CodedValueSource::Annotation);

        let constants = parse(
            "status",
            Some("状态：1-待处理（PENDING） 2-已完成（DONE）"),
            None,
            &java_type("Integer"),
        )
        .unwrap();
        assert_eq!(constants.source, CodedValueSource::ConstantReference);
        assert_eq!(constants.values[0].key.as_deref(), Some("PENDING"));
        assert_eq!(constants.values[1].key.as_deref(), Some("DONE"));

        assert_values(
            "ratio",
            "BigDecimal",
            "倍率取值：1.5-一级 2.5-二级",
            vec![(json!(1.5), "一级"), (json!(2.5), "二级")],
        );
        assert_values(
            "status",
            "Long",
            "状态：10-待处理 20-已完成",
            vec![(json!("10"), "待处理"), (json!("20"), "已完成")],
        );
    }

    #[test]
    fn syntax_guards_and_source_precedence_match_old_skill() {
        assert_values(
            "status",
            "Integer",
            "状态：1:待处理，2:已完成",
            vec![(json!(1), "待处理"), (json!(2), "已完成")],
        );
        assert_values(
            "status",
            "Integer",
            "状态：2/3/4-异常",
            vec![(json!(2), "异常"), (json!(3), "异常"), (json!(4), "异常")],
        );
        assert!(
            parse(
                "status",
                Some("状态：1-唯一值"),
                None,
                &java_type("Integer")
            )
            .is_none()
        );
        assert!(
            parse(
                "scene",
                Some("按钮场景\n1. first-code 第一项\n3. third-code 第三项"),
                None,
                &java_type("String")
            )
            .is_none()
        );
        assert!(
            parse(
                "scene",
                Some("按钮场景\n1. first-code 第一项\n- second-code 第二项"),
                None,
                &java_type("String")
            )
            .is_none()
        );

        let annotation_fallback = parse(
            "status",
            Some("普通说明"),
            Some("@Schema(description = \"状态：1-待处理 2-已完成\")"),
            &java_type("Integer"),
        )
        .unwrap();
        assert_eq!(annotation_fallback.source, CodedValueSource::Annotation);
        let comment_first = parse(
            "status",
            Some("状态：1-注释一 2-注释二"),
            Some("@Schema(description = \"状态：3-注解一 4-注解二\")"),
            &java_type("Integer"),
        )
        .unwrap();
        assert_eq!(comment_first.source, CodedValueSource::Comment);
        assert_eq!(comment_first.values[0].value, WireValue::Number(1));
    }

    #[test]
    fn fallback_keys_match_old_skill() {
        let values = parse(
            "statusCode",
            Some("状态码：-1-失败 2-成功"),
            None,
            &java_type("Integer"),
        )
        .unwrap();
        let values = with_fallback_keys("statusCode", &values.values);
        assert_eq!(values[0].key.as_deref(), Some("STATUS_CODE_NEGATIVE_1"));
        assert_eq!(values[1].key.as_deref(), Some("STATUS_CODE_2"));
    }
}
