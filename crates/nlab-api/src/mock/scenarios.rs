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

pub(super) fn selection_pattern(path: &str, query: &str, scenario: Option<&str>) -> Result<String> {
    let path = path_pattern(path)?;
    let key = encoded_literal(query);
    let other = format!("(?!{key}(?:=|&|$))[^&#]*");
    Ok(match scenario {
        Some(value) => format!(
            "{path}\\?(?:{other}&)*{key}={}(?:&{other})*$",
            encoded_literal(value)
        ),
        None => format!("{path}(?:\\?{other}(?:&{other})*)?$"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn matches(pattern: &str, url: &str) -> bool {
        jsonschema::is_valid(&json!({"type": "string", "pattern": pattern}), &json!(url))
    }

    #[test]
    fn query_selection_is_exact_order_independent_and_rejects_duplicates() {
        let done = selection_pattern("/orders/{id}", "__mock", Some("done")).unwrap();
        let base = selection_pattern("/orders/{id}", "__mock", None).unwrap();
        for query in [
            "__mock=done",
            "a=1&__mock=done",
            "__mock=done&a=1",
            "a=1&__mock=done&b=2",
            "%5F_mock=%64one",
        ] {
            assert!(
                matches(&done, &format!("https://example.test/orders/123?{query}")),
                "{query}"
            );
        }
        for query in [
            "__mock=done-extra",
            "__mock=undone",
            "other__mock=done",
            "q=__mock=done",
            "__mock=done&__mock=done",
            "__mock=bad&__mock=done",
            "__mock=done&__mock",
            "__mock=done&%5f_mock=done",
        ] {
            assert!(
                !matches(&done, &format!("https://example.test/orders/123?{query}")),
                "{query}"
            );
        }
        assert!(!matches(
            &done,
            "https://example.test/orders/123/extra?__mock=done"
        ));
        assert!(matches(&base, "https://example.test/orders/123?a=1"));
        for query in ["__mock=unknown", "__mock", "%5f_mock=unknown"] {
            assert!(!matches(
                &base,
                &format!("https://example.test/orders/123?{query}")
            ));
        }
    }
}
