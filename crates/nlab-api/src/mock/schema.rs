use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use fake::{
    Fake,
    faker::{address::raw::CityName, internet::raw::SafeEmail, name::raw::Name},
    locales::ZH_CN,
};
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use serde_json::{Map, Value, json};

use super::scenarios::Rules;

pub(super) struct Generator<'a> {
    pub document: &'a Value,
    pub rules: &'a Rules,
    pub rng: ChaCha8Rng,
    pub gaps: BTreeSet<String>,
}

impl Generator<'_> {
    pub fn generate(&mut self, schema: &Value) -> Result<Value> {
        self.value(schema, "", "", &mut BTreeSet::new())
    }

    fn value(
        &mut self,
        schema: &Value,
        pointer: &str,
        context: &str,
        visiting: &mut BTreeSet<String>,
    ) -> Result<Value> {
        for key in ["const", "example", "default"] {
            if let Some(value) = schema.get(key) {
                return Ok(value.clone());
            }
        }
        if let Some(value) = schema
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
        {
            self.gaps.insert(format!(
                "{pointer}: 使用首个合法枚举样例；状态与字段关系未推断"
            ));
            return Ok(value.clone());
        }
        let context = format!("{context} {}", schema["description"].as_str().unwrap_or(""));
        if let Some(reference) = schema["$ref"].as_str() {
            if visiting.len() >= 32 || !visiting.insert(reference.to_owned()) {
                return Ok(Value::Null);
            }
            let target = self
                .document
                .pointer(
                    reference
                        .strip_prefix('#')
                        .context("external OpenAPI references are unsupported")?,
                )
                .with_context(|| format!("OpenAPI reference missing: {reference}"))?;
            let result = self.value(target, pointer, &context, visiting);
            visiting.remove(reference);
            return result;
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(choices) = schema[key].as_array() {
                return self.value(
                    choices.first().context("empty schema alternatives")?,
                    pointer,
                    &context,
                    visiting,
                );
            }
        }
        if let Some(parts) = schema["allOf"].as_array() {
            let mut object = Map::new();
            for part in parts {
                let value = self.value(part, pointer, &context, visiting)?;
                object.extend(
                    value
                        .as_object()
                        .context("non-object allOf needs an explicit schema example")?
                        .clone(),
                );
            }
            return Ok(Value::Object(object));
        }
        let kind = schema["type"]
            .as_str()
            .or_else(|| {
                schema["type"]
                    .as_array()
                    .and_then(|v| v.iter().find_map(|v| v.as_str().filter(|s| *s != "null")))
            })
            .unwrap_or(if schema.get("properties").is_some() {
                "object"
            } else {
                "string"
            });
        match kind {
            "object" => {
                let mut object = Map::new();
                for (name, property) in schema["properties"].as_object().into_iter().flatten() {
                    let path = format!("{pointer}/{}", name.replace('~', "~0").replace('/', "~1"));
                    object.insert(
                        name.clone(),
                        self.value(property, &path, &context, visiting)?,
                    );
                }
                Ok(Value::Object(object))
            }
            "array" => {
                let minimum = schema["minItems"].as_u64().unwrap_or(0);
                let is_action = ["button", "action", "按钮", "操作码"].iter().any(|word| {
                    format!("{pointer} {}", schema["description"].as_str().unwrap_or(""))
                        .to_lowercase()
                        .contains(word)
                });
                let recursive = schema["items"]["$ref"]
                    .as_str()
                    .is_some_and(|r| visiting.contains(r));
                let count = if recursive && minimum == 0 {
                    0
                } else if is_action {
                    self.gaps.insert(format!(
                        "{pointer}: 按钮规则未知，暂未覆盖；空列表仅用于占位"
                    ));
                    0
                } else {
                    2.max(minimum)
                        .min(schema["maxItems"].as_u64().unwrap_or(2.max(minimum)))
                };
                if count > 1000 {
                    bail!("{pointer}: minItems exceeds mock generation limit 1000");
                }
                (0..count)
                    .map(|index| {
                        self.value(
                            &schema["items"],
                            &format!("{pointer}/{index}"),
                            &context,
                            visiting,
                        )
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(Value::Array)
            }
            "integer" | "number" => {
                if pointer.to_lowercase().ends_with("status") {
                    self.gaps.insert(format!(
                        "{pointer}: 契约未声明状态枚举，数值仅为类型合法的基础样例"
                    ));
                }
                let mut value = if pointer.to_lowercase().ends_with("id") {
                    self.rng.random_range(100_000..=999_999) as f64
                } else {
                    1.0
                };
                if let Some(minimum) = schema["minimum"].as_f64() {
                    value = f64::max(value, minimum);
                }
                if let Some(minimum) = schema["exclusiveMinimum"].as_f64() {
                    value = f64::max(value, minimum + 1.0);
                }
                if let Some(maximum) = schema["maximum"].as_f64() {
                    value = f64::min(value, maximum);
                }
                if let Some(maximum) = schema["exclusiveMaximum"].as_f64() {
                    value = f64::min(value, maximum - 1.0);
                }
                if let Some(multiple) = schema["multipleOf"].as_f64().filter(|n| *n > 0.0) {
                    value = (value / multiple).ceil() * multiple;
                }
                if kind == "integer" {
                    Ok(json!(value.ceil() as i64))
                } else {
                    Ok(json!(value))
                }
            }
            "boolean" => Ok(json!(false)),
            "null" => Ok(Value::Null),
            "string" => {
                let mut value = self.semantic(pointer, &context, schema, None)?;
                let min = schema["minLength"].as_u64().unwrap_or(0) as usize;
                if min > 10000 {
                    bail!("{pointer}: minLength exceeds mock generation limit 10000");
                }
                while value.chars().count() < min {
                    value.push('样');
                }
                if let Some(max) = schema["maxLength"].as_u64() {
                    value = value.chars().take(max as usize).collect();
                }
                Ok(Value::String(value))
            }
            other => bail!("unsupported mock schema type: {other}"),
        }
    }

    pub fn apply_generators(
        &mut self,
        data: &mut Value,
        generators: &BTreeMap<String, String>,
    ) -> Result<()> {
        for (pointer, generator) in generators {
            let target = data
                .pointer_mut(pointer)
                .with_context(|| format!("mock field does not exist: {pointer}"))?;
            *target = Value::String(self.semantic(pointer, "", &Value::Null, Some(generator))?);
        }
        Ok(())
    }

    fn semantic(
        &mut self,
        pointer: &str,
        context: &str,
        schema: &Value,
        generator: Option<&str>,
    ) -> Result<String> {
        let field = pointer
            .rsplit('/')
            .find(|part| !part.bytes().all(|b| b.is_ascii_digit()))
            .unwrap_or(pointer)
            .to_lowercase();
        let format = schema["format"].as_str().unwrap_or("");
        let local =
            format!("{field} {}", schema["description"].as_str().unwrap_or("")).to_lowercase();
        let words = format!("{pointer} {context}").to_lowercase();
        let contains = |terms: &[&str]| terms.iter().any(|term| words.contains(term));
        let local_contains = |terms: &[&str]| terms.iter().any(|term| local.contains(term));
        let inferred = if field.contains("status") {
            "statusLabel"
        } else if format == "date-time" || field.ends_with("time") {
            "dateTime"
        } else if format == "date" || field.ends_with("date") {
            "date"
        } else if format == "email" || field.contains("email") {
            "email"
        } else if format == "uuid" {
            "uuid"
        } else if field.contains("phone") || field.contains("mobile") {
            "phone"
        } else if local_contains(&["image", "picture", "avatar", "图片", "主图"])
            || field.contains("img")
        {
            "image"
        } else if format == "uri" || format == "url" || field.ends_with("url") {
            "url"
        } else if field.ends_with("id") || field.ends_with("no") {
            "identifier"
        } else if field.contains("name")
            || field.contains("title")
            || local_contains(&["名称", "姓名", "标题"])
        {
            if field.contains("user")
                || field.contains("contact")
                || field.contains("operator")
                || local_contains(&["姓名", "联系人", "操作人"])
            {
                "personName"
            } else if local_contains(&["goods", "product", "商品", "产品", "货品"]) {
                "productName"
            } else if pointer.to_lowercase().contains("contact")
                || pointer.to_lowercase().contains("user")
            {
                "personName"
            } else if contains(&["goods", "product", "商品", "产品", "自行车"]) {
                "productName"
            } else if contains(&["person", "customer", "姓名", "联系人", "用户", "客户"]) {
                "personName"
            } else {
                "label"
            }
        } else if local_contains(&["address", "地址"]) {
            "address"
        } else if field.contains("desc") || field.contains("remark") {
            "description"
        } else {
            "label"
        };
        if inferred == "statusLabel" && generator.is_none() {
            self.gaps
                .insert(format!("{pointer}: 状态文案尚未确认，仅使用占位内容"));
        }
        Ok(match generator.unwrap_or(inferred) {
            "personName" => Name(ZH_CN).fake_with_rng(&mut self.rng),
            "email" => SafeEmail(ZH_CN).fake_with_rng(&mut self.rng),
            "address" => format!(
                "{}中山路{}号",
                CityName(ZH_CN).fake_with_rng::<String, _>(&mut self.rng),
                self.rng.random_range(1..200)
            ),
            "phone" => format!("138{:08}", self.rng.random_range(0..100_000_000)),
            "productName" => {
                ["山地自行车", "公路自行车", "折叠自行车"][self.rng.random_range(0..3)].to_owned()
            }
            "description" => "日常使用，外观有轻微使用痕迹，功能待核验".to_owned(),
            "statusLabel" => "状态文案待确认".to_owned(),
            "image" => "https://placehold.co/640x480/png?text=Product+illustration".to_owned(),
            "url" => "https://example.com/preview".to_owned(),
            "dateTime" => self.rules.reference_date.clone(),
            "date" => self.rules.reference_date[..10].to_owned(),
            "identifier" => self.rng.random_range(100_000..=999_999).to_string(),
            "uuid" => format!(
                "{:08x}-0000-4000-8000-{:012x}",
                self.rng.random::<u32>(),
                self.rng.random::<u32>()
            ),
            "label" => "样例内容".to_owned(),
            other => bail!("unknown mock generator: {other}"),
        })
    }
}

pub(super) fn validator(schema: &Value, document: &Value) -> Result<jsonschema::Validator> {
    // Keep local component references resolvable; network/file resolution is disabled.
    let mut root = json!({"allOf": [schema], "components": document["components"]});
    normalize(&mut root)?;
    jsonschema::options()
        .should_validate_formats(true)
        .build(&root)
        .map_err(|error| anyhow::anyhow!("invalid response schema: {error}"))
}

fn normalize(value: &mut Value) -> Result<()> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    // Visit schema positions only. Example values and properties named nullable are data.
    for keyword in ["properties", "patternProperties", "$defs", "definitions"] {
        if let Some(properties) = object.get_mut(keyword).and_then(Value::as_object_mut) {
            for property in properties.values_mut() {
                normalize(property)?;
            }
        }
    }
    if let Some(components) = object
        .get_mut("components")
        .and_then(|v| v.get_mut("schemas"))
        .and_then(Value::as_object_mut)
    {
        for schema in components.values_mut() {
            normalize(schema)?;
        }
    }
    for keyword in [
        "additionalProperties",
        "items",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "unevaluatedProperties",
        "unevaluatedItems",
    ] {
        if let Some(schema) = object.get_mut(keyword) {
            normalize(schema)?;
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(schemas) = object.get_mut(keyword).and_then(Value::as_array_mut) {
            for schema in schemas {
                normalize(schema)?;
            }
        }
    }
    if object.get("nullable") == Some(&Value::Bool(true)) {
        object.remove("nullable");
        let inner = Value::Object(std::mem::take(object));
        object.insert("anyOf".into(), json!([inner, {"type":"null"}]));
    }
    Ok(())
}
