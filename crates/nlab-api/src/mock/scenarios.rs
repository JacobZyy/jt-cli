use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Rules {
    pub version: u8,
    pub locale: String,
    pub reference_date: String,
    pub query: String,
    #[serde(default)]
    pub operations: BTreeMap<String, Operation>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            version: 1,
            locale: "zh_CN".into(),
            reference_date: "2026-09-08T00:00:00Z".into(),
            query: "__mock".into(),
            operations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Operation {
    #[serde(default)]
    pub coverage: Coverage,
    #[serde(default)]
    pub base: BTreeMap<String, Value>,
    #[serde(default)]
    pub generators: BTreeMap<String, String>,
    #[serde(default)]
    pub scenarios: BTreeMap<String, Scenario>,
    pub default_scenario: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub gaps: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Coverage {
    #[default]
    Partial,
    Complete,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    pub values: BTreeMap<String, Value>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub gaps: Vec<String>,
}

impl Rules {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.locale != "zh_CN" {
            bail!("mock rules require version 1 and locale zh_CN");
        }
        jsonschema::options()
            .should_validate_formats(true)
            .build(&serde_json::json!({"type":"string", "format":"date-time"}))?
            .validate(&Value::String(self.reference_date.clone()))
            .map_err(|error| anyhow::anyhow!("invalid referenceDate: {error}"))?;
        validate_name(&self.query)?;
        for (key, operation) in &self.operations {
            if let Some(default) = &operation.default_scenario {
                if !operation.scenarios.contains_key(default) {
                    bail!("{key}: defaultScenario does not name a scenario");
                }
            }
            for name in operation.scenarios.keys() {
                validate_name(name)?;
                if matches!(name.as_str(), "base" | "default") {
                    bail!("{key}: scenario names base and default are reserved");
                }
            }
            if operation.coverage == Coverage::Complete
                && (operation.scenarios.is_empty()
                    || operation.sources.is_empty()
                    || !operation.gaps.is_empty()
                    || operation.scenarios.values().any(|s| !s.gaps.is_empty()))
            {
                bail!("{key}: complete coverage requires scenarios, sources and no declared gaps");
            }
        }
        Ok(())
    }
}

impl Operation {
    pub fn tier(&self) -> u8 {
        if self.scenarios.is_empty() {
            1
        } else if self.coverage == Coverage::Complete {
            3
        } else {
            2
        }
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        bail!(
            "mock query and scenario names require nonempty ASCII letters, digits, _ or -: {name}"
        );
    }
    Ok(())
}

pub(super) fn apply_values(data: &mut Value, values: &BTreeMap<String, Value>) -> Result<()> {
    for (pointer, value) in values {
        // Replacements cannot introduce properties absent from the generated contract object.
        let target = data
            .pointer_mut(pointer)
            .with_context(|| format!("mock field does not exist: {pointer}"))?;
        *target = value.clone();
    }
    Ok(())
}

fn encoded_literal(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            let hex = format!("{byte:02X}")
                .chars()
                .map(|c| {
                    if c.is_ascii_alphabetic() {
                        format!("[{}{}]", c.to_ascii_lowercase(), c)
                    } else {
                        c.to_string()
                    }
                })
                .collect::<String>();
            format!("(?:{}|%{hex})", regex::escape(&(byte as char).to_string()))
        })
        .collect()
}

pub(super) fn path_pattern(path: &str) -> Result<String> {
    if !path.starts_with('/') || path.contains(['?', '#']) || path.chars().any(char::is_whitespace)
    {
        bail!("unsupported OpenAPI mock path: {path}");
    }
    // OpenAPI path parameters match one nonempty segment, never a path prefix.
    let parameters = regex::Regex::new(r"\{[^/{}]+\}").expect("constant regex");
    let mut escaped = String::new();
    let mut end = 0;
    for capture in parameters.find_iter(path) {
        escaped.push_str(&regex::escape(&path[end..capture.start()]));
        escaped.push_str("[^/?#]+");
        end = capture.end();
    }
    escaped.push_str(&regex::escape(&path[end..]));
    Ok(format!("^https?://[^/?#]+{escaped}"))
}

pub(super) fn request_pattern(path: &str) -> Result<String> {
    let regular = path_pattern(path)?;
    if path.contains('{') {
        Ok(format!("/{regular}(?:\\?[^#]*)?$/"))
    } else {
        Ok(format!("$http*://*{path}"))
    }
}

pub(super) fn selector_pattern(query: &str, scenario: Option<&str>) -> String {
    let key = query;
    let suffix = scenario.map_or_else(
        || "(?:=|&|$)".to_owned(),
        |value| format!("={}(?:&|$)", value),
    );
    format!("^[^?]*\\?(?:[^&]*&)*{key}{suffix}")
}

pub(super) fn duplicate_pattern(query: &str) -> String {
    let key = query;
    format!("^[^?]*\\?(?:[^&]*&)*{key}(?:=[^&]*)?&(?:[^&]*&)*{key}(?:=|&|$)")
}

pub(super) fn encoded_selector_pattern(query: &str) -> String {
    format!(
        "^[^?]*\\?(?:[^&]*&)*(?!{}(?:=|&|$)){}(?:=|&|$)",
        regex::escape(query),
        encoded_literal(query)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn matches(pattern: &str, url: &str) -> bool {
        jsonschema::is_valid(&json!({"type":"string","pattern":pattern}), &json!(url))
    }
    #[test]
    fn query_filters_keep_order_encoding_and_duplicate_boundaries() {
        let selected = selector_pattern("__mock", Some("done"));
        let present = selector_pattern("__mock", None);
        let duplicate = duplicate_pattern("__mock");
        let encoded = encoded_selector_pattern("__mock");
        for query in [
            "%5F_mock=done",
            "__mock=done&%5f_mock=done",
            "%5f%5f%6d%6f%63%6b=done",
        ] {
            assert!(matches(
                &encoded,
                &format!("https://example.test/api?{query}")
            ));
        }
        assert!(!matches(&encoded, "https://example.test/api?__mock=done"));
        for query in [
            "__mock=done",
            "a=1&__mock=done",
            "__mock=done&a=1",
            "a=1&__mock=done&b=2",
        ] {
            let url = format!("https://example.test/orders/123?{query}");
            assert!(matches(&selected, &url));
            assert!(!matches(&duplicate, &url));
        }
        for query in [
            "__mock=done-extra",
            "__mock=undone",
            "other__mock=done",
            "q=__mock=done",
            "q=?__mock=done",
        ] {
            assert!(
                !matches(
                    &selected,
                    &format!("https://example.test/orders/123?{query}")
                ),
                "{query}"
            );
        }
        for query in [
            "__mock=done&__mock=done",
            "__mock=bad&__mock=done",
            "__mock=done&__mock",
        ] {
            assert!(
                matches(
                    &duplicate,
                    &format!("https://example.test/orders/123?{query}")
                ),
                "{query}"
            );
        }
        for query in ["__mock=unknown", "__mock"] {
            assert!(matches(
                &present,
                &format!("https://example.test/orders/123?{query}")
            ));
        }
        assert!(!matches(
            &present,
            "https://example.test/orders/123?q=?__mock=done"
        ));
        assert_eq!(
            request_pattern("/orders/detail").unwrap(),
            "$http*://*/orders/detail"
        );
        assert!(request_pattern("/orders/{id}").unwrap().starts_with('/'));
    }
}
