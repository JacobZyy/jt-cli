use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use super::coded_values;
use super::config::ProjectConfig;
use super::layout::{api_output_path, join_path, type_output_path};
use super::model::{
    CodedValue, ContractIr, Operation, ProvenanceStatus, RouteSource, RouteStatus, Schema,
    SemanticPatch, TypeRef, WireValue,
};
use super::naming::{
    fqn_seed, shortest_unique_names, shortest_unique_names_avoiding, without_interface_prefix,
};

pub struct OpenApiArtifact {
    pub source: String,
    pub paths: usize,
    pub schemas: usize,
}

pub fn generate(ir: &ContractIr, config: &ProjectConfig) -> Result<OpenApiArtifact> {
    let names = schema_names(&ir.schemas);
    let aliases_by_operation = response_aliases(ir, &names);
    let mut components = Map::new();
    for (fqn, schema) in &ir.schemas {
        components.insert(
            names[fqn].clone(),
            schema_object(schema, &names, None, &HashMap::new()),
        );
    }

    let mut paths = Map::new();
    let mut contracts = Map::new();
    for operation in &ir.operations {
        let aliases = aliases_by_operation
            .get(&operation.key)
            .cloned()
            .unwrap_or_default();
        let mut alias_entries = aliases.iter().collect::<Vec<_>>();
        alias_entries.sort_by(|left, right| left.0.cmp(right.0));
        for (fqn, alias) in alias_entries {
            let schema = &ir.schemas[fqn];
            components.insert(
                alias.clone(),
                schema_object(schema, &names, Some(operation), &aliases),
            );
        }
        let operation_value = operation_object(operation, &names, &aliases, config)?;
        let mut path_item = Map::new();
        path_item.insert(
            operation.route.method.to_ascii_lowercase(),
            operation_value.clone(),
        );
        paths.insert(operation.route.path.clone(), Value::Object(path_item));
        contracts.insert(operation.key.clone(), operation_value);
    }

    let mut root = Map::new();
    root.insert("openapi".to_owned(), Value::String("3.1.0".to_owned()));
    root.insert(
        "info".to_owned(),
        json!({
            "title": format!("{} nlab API", ir.target.app_name),
            "version": "1.0.0"
        }),
    );
    root.insert("paths".to_owned(), Value::Object(paths));
    root.insert(
        "components".to_owned(),
        json!({ "schemas": Value::Object(components) }),
    );
    root.insert(
        "x-nlab".to_owned(),
        json!({
            "appName": ir.target.app_name,
            "branch": ir.target.branch,
            "commit": ir.target.commit,
            "codegraphVersion": ir.target.codegraph_version,
            "codegraphExtractionVersion": ir.target.codegraph_extraction_version,
            "mode": "full",
            "generator": format!("jt/{}", env!("CARGO_PKG_VERSION"))
        }),
    );
    root.insert("x-nlab-contracts".to_owned(), Value::Object(contracts));
    let document = Value::Object(root);
    validate(&document, ir.operations.len())?;
    let source = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let paths = document["paths"].as_object().map_or(0, Map::len);
    let schemas = document["components"]["schemas"]
        .as_object()
        .map_or(0, Map::len);
    Ok(OpenApiArtifact {
        source,
        paths,
        schemas,
    })
}

pub fn validate(document: &Value, expected_operations: usize) -> Result<()> {
    if document["openapi"] != "3.1.0" {
        bail!("invalid OpenAPI version");
    }
    let paths = document["paths"]
        .as_object()
        .context("OpenAPI paths missing")?;
    let schemas = document["components"]["schemas"]
        .as_object()
        .context("OpenAPI schemas missing")?;
    let contracts = document["x-nlab-contracts"]
        .as_object()
        .context("OpenAPI x-nlab-contracts missing")?;
    if paths.len() != expected_operations || contracts.len() != expected_operations {
        bail!(
            "OpenAPI operation count mismatch: paths={} contracts={} expected={expected_operations}",
            paths.len(),
            contracts.len()
        );
    }
    let mut missing = BTreeSet::new();
    visit_refs(document, &mut |reference| {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            let decoded = name.replace("~1", "/").replace("~0", "~");
            if !schemas.contains_key(&decoded) {
                missing.insert(decoded);
            }
        }
    });
    if !missing.is_empty() {
        bail!(
            "OpenAPI schema references missing: {}",
            missing.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}

fn operation_object(
    operation: &Operation,
    names: &BTreeMap<String, String>,
    aliases: &HashMap<String, String>,
    config: &ProjectConfig,
) -> Result<Value> {
    let mut value = Map::new();
    value.insert(
        "operationId".to_owned(),
        Value::String(format!(
            "{}_{}",
            operation.facade_name, operation.method_name
        )),
    );
    value.insert(
        "summary".to_owned(),
        Value::String(
            operation
                .description
                .clone()
                .unwrap_or_else(|| operation.method_name.clone()),
        ),
    );
    value.insert(
        "tags".to_owned(),
        json!([operation
            .service
            .as_ref()
            .map(|service| service.class_name.as_str())
            .unwrap_or(&operation.facade_name)]),
    );
    value.insert("x-nlab-operation-key".to_owned(), json!(operation.key));
    value.insert(
        "x-nlab-method-name".to_owned(),
        json!(operation.method_name),
    );
    value.insert("x-nlab-facade".to_owned(), json!(operation.facade_name));
    value.insert("x-nlab-facade-fqn".to_owned(), json!(operation.facade_fqn));
    value.insert(
        "x-nlab-contract-source".to_owned(),
        json!(operation.contract_source),
    );
    value.insert(
        "x-nlab-route-status".to_owned(),
        json!(route_status(operation.route.status)),
    );
    value.insert(
        "x-nlab-route-source".to_owned(),
        json!(route_source(operation.route.source)),
    );
    value.insert("x-nlab-http-path".to_owned(), json!(operation.route.path));
    value.insert(
        "x-nlab-http-method".to_owned(),
        json!(operation.route.method.to_ascii_lowercase()),
    );
    if let Some(host) = &operation.route.host {
        value.insert("x-nlab-http-host".to_owned(), json!(host));
    }
    value.insert(
        "x-nlab-api-output".to_owned(),
        json!(join_path(
            &config.frontend.layout.implementation_dir,
            &api_output_path(operation, &config.backend.contract_roots)?
        )),
    );
    value.insert(
        "x-nlab-type-output".to_owned(),
        json!(join_path(
            &config.frontend.layout.types_dir,
            &type_output_path(operation, &config.backend.contract_roots)?
        )),
    );
    if let Some(service) = &operation.service {
        value.insert("x-nlab-service-class".to_owned(), json!(service.class_name));
        value.insert("x-nlab-service-fqn".to_owned(), json!(service.class_fqn));
        value.insert(
            "x-nlab-service-method".to_owned(),
            json!(service.method_name),
        );
        value.insert(
            "x-nlab-service-source".to_owned(),
            json!(service.source_path),
        );
    }
    value.insert(
        "x-nlab-semantic-patches".to_owned(),
        Value::Array(
            operation
                .semantic_patches
                .iter()
                .map(semantic_patch)
                .collect(),
        ),
    );
    if !operation.warnings.is_empty() {
        value.insert("x-nlab-warnings".to_owned(), json!(operation.warnings));
    }
    if let Some(request) = &operation.request {
        value.insert(
            "x-nlab-request-type".to_owned(),
            json!(request.render_java()),
        );
        value.insert(
            "requestBody".to_owned(),
            json!({
                "required": true,
                "content": {
                    "application/json": {
                        "schema": type_schema(request, names, &HashMap::new())
                    }
                }
            }),
        );
    }
    value.insert(
        "x-nlab-response-type".to_owned(),
        json!(operation.response.render_java()),
    );
    value.insert(
        "responses".to_owned(),
        json!({
            "200": {
                "description": "OK",
                "content": {
                    "application/json": {
                        "schema": type_schema(&operation.response, names, aliases)
                    }
                }
            }
        }),
    );
    Ok(Value::Object(value))
}

fn schema_object(
    schema: &Schema,
    names: &BTreeMap<String, String>,
    operation: Option<&Operation>,
    aliases: &HashMap<String, String>,
) -> Value {
    let patches = operation
        .map(|operation| {
            operation
                .semantic_patches
                .iter()
                .filter(|patch| {
                    patch.target.schema_fqn == schema.fqn
                        && patch.status == ProvenanceStatus::Closed
                })
                .map(|patch| (patch.target.field_name.as_str(), patch))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in &schema.fields {
        let mut property = type_schema(&field.java_type, names, aliases);
        if let Some(description) = &field.description {
            property
                .as_object_mut()
                .expect("schema object")
                .insert("description".to_owned(), json!(description));
        }
        if let Some(patch) = patches.get(field.name.as_str()) {
            apply_enum(&mut property, &patch.values);
        } else if let Some(linked) = &field.linked_enum {
            apply_enum(&mut property, &linked.values);
            property.as_object_mut().expect("schema object").insert(
                "x-nlab-linked-enum".to_owned(),
                json!({
                    "enumFqn": linked.enum_fqn,
                    "enumSource": linked.enum_source,
                    "accessor": linked.accessor,
                }),
            );
        } else if let Some(values) = &field.declared_values {
            apply_enum(
                &mut property,
                &coded_values::with_fallback_keys(&field.name, &values.values),
            );
            property
                .as_object_mut()
                .expect("schema object")
                .insert("x-nlab-known-values".to_owned(), json!(values));
        }
        properties.insert(field.name.clone(), property);
        if !field.optional {
            required.push(Value::String(field.name.clone()));
        }
    }
    let mut value = Map::new();
    value.insert("type".to_owned(), json!("object"));
    if let Some(description) = &schema.description {
        value.insert("description".to_owned(), json!(description));
    }
    value.insert("properties".to_owned(), Value::Object(properties));
    if !required.is_empty() {
        value.insert("required".to_owned(), Value::Array(required));
    }
    value.insert("additionalProperties".to_owned(), Value::Bool(false));
    value.insert("x-nlab-schema-fqn".to_owned(), json!(schema.fqn));
    value.insert("x-nlab-source".to_owned(), json!(schema.source_path));
    Value::Object(value)
}

fn type_schema(
    type_ref: &TypeRef,
    names: &BTreeMap<String, String>,
    aliases: &HashMap<String, String>,
) -> Value {
    if type_ref.array_depth > 0 {
        let mut item = type_ref.clone();
        item.array_depth -= 1;
        return json!({ "type": "array", "items": type_schema(&item, names, aliases) });
    }
    let simple = type_ref.simple_name();
    if is_collection(simple) {
        return json!({
            "type": "array",
            "items": type_ref.arguments.first()
                .map(|item| type_schema(item, names, aliases))
                .unwrap_or_else(|| json!({}))
        });
    }
    if matches!(simple, "PageList" | "Page" | "PageResult") {
        let item = type_ref
            .arguments
            .last()
            .map(|item| type_schema(item, names, aliases))
            .unwrap_or_else(|| json!({}));
        return json!({
            "type": "object",
            "properties": {
                "totalNum": { "type": "number" },
                "list": { "type": "array", "items": item }
            },
            "required": ["totalNum", "list"],
            "additionalProperties": false,
            "x-nlab-java-type": simple
        });
    }
    if matches!(simple, "Map" | "HashMap" | "LinkedHashMap" | "TreeMap") {
        let value = type_ref
            .arguments
            .last()
            .map(|value| type_schema(value, names, aliases))
            .unwrap_or_else(|| json!({}));
        return json!({ "type": "object", "additionalProperties": value });
    }
    if simple == "Optional" {
        return type_ref
            .arguments
            .first()
            .map(|value| type_schema(value, names, aliases))
            .unwrap_or_else(|| json!({}));
    }
    let fqn = type_ref.name.replace("::", ".");
    if let Some(name) = aliases.get(&fqn).or_else(|| names.get(&fqn)) {
        return json!({ "$ref": format!("#/components/schemas/{name}") });
    }
    match simple {
        "String" | "CharSequence" | "char" | "Character" => json!({ "type": "string" }),
        "Long" | "long" | "BigInteger" => {
            json!({ "type": "string", "x-nlab-java-type": simple })
        }
        "Integer" | "int" | "Short" | "short" | "Byte" | "byte" | "Double" | "double" | "Float"
        | "float" | "BigDecimal" => json!({ "type": "number" }),
        "Boolean" | "boolean" => json!({ "type": "boolean" }),
        "Date" | "LocalDate" | "LocalDateTime" | "Instant" | "Timestamp" => {
            json!({ "type": "string" })
        }
        "Void" | "void" => json!({ "type": "null" }),
        _ => json!({}),
    }
}

fn apply_enum(schema: &mut Value, values: &[CodedValue]) {
    let object = schema.as_object_mut().expect("enum schema object");
    object.insert(
        "enum".to_owned(),
        Value::Array(values.iter().map(|item| wire_json(&item.value)).collect()),
    );
    object.insert(
        "x-enum-varnames".to_owned(),
        json!(
            values
                .iter()
                .map(|item| item
                    .key
                    .clone()
                    .unwrap_or_else(|| neutral_enum_name(&item.value)))
                .collect::<Vec<_>>()
        ),
    );
    object.insert(
        "x-enum-descriptions".to_owned(),
        json!(
            values
                .iter()
                .map(|item| item.label.clone())
                .collect::<Vec<_>>()
        ),
    );
}

fn semantic_patch(patch: &SemanticPatch) -> Value {
    let mut value = Map::new();
    value.insert("target".to_owned(), json!(patch.target));
    value.insert("status".to_owned(), json!(patch.status));
    if let Some(enum_fqn) = &patch.enum_fqn {
        value.insert("enumFqn".to_owned(), json!(enum_fqn));
    }
    if let Some(enum_source) = &patch.enum_source {
        value.insert("enumSource".to_owned(), json!(enum_source));
    }
    if let Some(accessor) = &patch.accessor {
        value.insert("accessor".to_owned(), json!(accessor));
    }
    if !patch.values.is_empty() {
        value.insert("values".to_owned(), json!(patch.values));
    }
    if !patch.evidence.is_empty() {
        value.insert("evidence".to_owned(), json!(patch.evidence));
    }
    if let Some(warning) = &patch.warning {
        value.insert("warning".to_owned(), json!(warning));
    }
    Value::Object(value)
}

fn response_aliases(
    ir: &ContractIr,
    names: &BTreeMap<String, String>,
) -> HashMap<String, HashMap<String, String>> {
    let mut seeds = BTreeMap::new();
    let mut reachable_by_operation = HashMap::new();
    for operation in &ir.operations {
        if !operation
            .semantic_patches
            .iter()
            .any(|patch| patch.status == ProvenanceStatus::Closed)
        {
            continue;
        }
        let reachable = reachable_schemas(&operation.response, &ir.schemas);
        for fqn in &reachable {
            seeds.insert(
                alias_symbol(&operation.key, fqn),
                vec![
                    without_interface_prefix(&operation.facade_name).to_owned(),
                    operation.method_name.clone(),
                    ir.schemas[fqn].name.clone(),
                ],
            );
        }
        reachable_by_operation.insert(operation.key.clone(), reachable);
    }
    let reserved = names.values().cloned().collect::<BTreeSet<_>>();
    let alias_names = shortest_unique_names_avoiding(&seeds, &reserved);
    reachable_by_operation
        .into_iter()
        .map(|(operation_key, reachable)| {
            let aliases = reachable
                .into_iter()
                .map(|fqn| {
                    let name = alias_names[&alias_symbol(&operation_key, &fqn)].clone();
                    (fqn, name)
                })
                .collect();
            (operation_key, aliases)
        })
        .collect()
}

fn alias_symbol(operation_key: &str, fqn: &str) -> String {
    format!("{operation_key}:{fqn}")
}

pub(crate) fn reachable_schemas(
    type_ref: &TypeRef,
    schemas: &BTreeMap<String, Schema>,
) -> BTreeSet<String> {
    fn visit(value: &TypeRef, schemas: &BTreeMap<String, Schema>, found: &mut BTreeSet<String>) {
        let fqn = value.name.replace("::", ".");
        if let Some(schema) = schemas.get(&fqn) {
            if found.insert(fqn) {
                for field in &schema.fields {
                    visit(&field.java_type, schemas, found);
                }
            }
        }
        for argument in &value.arguments {
            visit(argument, schemas, found);
        }
    }
    let mut found = BTreeSet::new();
    visit(type_ref, schemas, &mut found);
    found
}

pub(crate) fn schema_names(schemas: &BTreeMap<String, Schema>) -> BTreeMap<String, String> {
    shortest_unique_names(
        &schemas
            .keys()
            .map(|fqn| (fqn.clone(), fqn_seed(fqn)))
            .collect(),
    )
}

fn neutral_enum_name(value: &WireValue) -> String {
    let value = match value {
        WireValue::String(value) => value.clone(),
        WireValue::Number(value) => format!("VALUE_{value}"),
        WireValue::Decimal(value) => format!("VALUE_{}", value.to_string().replace('.', "_")),
    };
    let mut result = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    while result.contains("__") {
        result = result.replace("__", "_");
    }
    if result
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
    {
        result.insert_str(0, "VALUE_");
    }
    result.trim_matches('_').to_owned()
}

fn wire_json(value: &WireValue) -> Value {
    match value {
        WireValue::String(value) => json!(value),
        WireValue::Number(value) => json!(value),
        WireValue::Decimal(value) => json!(value),
    }
}

pub(crate) fn sanitize_identifier(value: &str) -> String {
    let mut result = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '$') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if !result
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
    {
        result.insert_str(0, "Schema_");
    }
    result
}

fn is_collection(name: &str) -> bool {
    matches!(
        name,
        "List" | "Set" | "Collection" | "ArrayList" | "LinkedList" | "HashSet" | "Iterable"
    )
}

fn route_status(status: RouteStatus) -> &'static str {
    match status {
        RouteStatus::Placeholder => "placeholder",
        RouteStatus::Resolved => "resolved",
        RouteStatus::Cached => "cached",
        RouteStatus::QueryFailed => "query-failed",
    }
}

fn route_source(source: RouteSource) -> &'static str {
    match source {
        RouteSource::Placeholder => "placeholder",
        RouteSource::Zgateway => "zgateway",
        RouteSource::Cache => "cache",
    }
}

fn visit_refs(value: &Value, visitor: &mut impl FnMut(&str)) {
    match value {
        Value::Array(values) => values.iter().for_each(|value| visit_refs(value, visitor)),
        Value::Object(values) => values.iter().for_each(|(key, value)| {
            if key == "$ref" {
                if let Some(reference) = value.as_str() {
                    visitor(reference);
                }
            } else {
                visit_refs(value, visitor);
            }
        }),
        _ => {}
    }
}
