use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use super::config::ProjectConfig;
use super::layout::{api_output_path, join_path, type_output_path};
use super::model::{
    CodedValue, ContractIr, FieldSource, InputLocation, Operation, RouteSource, RouteStatus,
    Schema, SemanticPatch, TypeRef, WireValue,
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
    let plan = schema_plan(ir);
    let aliases_by_operation = &plan.aliases;
    let empty_aliases = HashMap::new();
    let mut components = Map::new();
    for variant in &plan.variants {
        let operation = variant.operation.map(|index| &ir.operations[index]);
        let aliases = operation
            .and_then(|operation| {
                aliases_by_operation.get(&variant.source.operation_key(operation))
            })
            .unwrap_or(&empty_aliases);
        components.insert(
            variant.name.clone(),
            schema_object(
                &ir.schemas[&variant.fqn],
                &names,
                operation,
                variant.source,
                aliases,
                &BTreeMap::new(),
                variant.optional_fields,
            ),
        );
    }

    let mut paths = Map::new();
    let mut contracts = Map::new();
    for operation in &ir.operations {
        let aliases = aliases_by_operation
            .get(&operation.key)
            .cloned()
            .unwrap_or_default();
        let request_aliases = aliases_by_operation
            .get(&FieldSource::Request.operation_key(operation))
            .cloned()
            .unwrap_or_default();
        let operation_value = operation_object(
            operation,
            &ir.schemas,
            &names,
            &aliases,
            &request_aliases,
            config,
        )?;
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
    schemas: &BTreeMap<String, Schema>,
    names: &BTreeMap<String, String>,
    aliases: &HashMap<String, String>,
    request_aliases: &HashMap<String, String>,
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
    }
    let mut query = Vec::new();
    let mut body = None;
    if operation.request_arguments.is_empty() {
        body = operation.request.as_ref().map(|request| {
            operation_schema(
                request,
                schemas,
                names,
                operation,
                FieldSource::Request,
                request_aliases,
                true,
            )
        });
    } else {
        let mut body_fields = Map::new();
        for argument in &operation.request_arguments {
            let schema = operation_schema(
                &argument.java_type,
                schemas,
                names,
                operation,
                FieldSource::Request,
                request_aliases,
                true,
            );
            match (argument.location, argument.name.as_deref()) {
                (InputLocation::Query, Some(name)) => {
                    query.push(
                        json!({"name": name, "in": "query", "required": false, "schema": schema}),
                    );
                }
                (InputLocation::Query, None) => {
                    if let Some(root) = schemas.get(&argument.java_type.name.replace("::", ".")) {
                        let object = schema_object(
                            root,
                            names,
                            Some(operation),
                            FieldSource::Request,
                            request_aliases,
                            &root.bindings_for(&argument.java_type),
                            true,
                        );
                        for (name, field) in object["properties"].as_object().into_iter().flatten()
                        {
                            query.push(json!({"name": name, "in": "query", "required": false, "schema": field}));
                        }
                    } else {
                        query.push(json!({
                            "name": argument.java_name, "in": "query", "required": false,
                            "style": "form", "explode": true, "schema": schema,
                        }));
                    }
                }
                (InputLocation::Body, Some(name)) => {
                    body_fields.insert(name.to_owned(), schema);
                }
                (InputLocation::Body, None) => body = Some(schema),
            }
        }
        if !body_fields.is_empty() {
            body = Some(json!({
                "type": "object", "properties": body_fields, "additionalProperties": false,
            }));
        }
    }
    if !query.is_empty() {
        value.insert("parameters".to_owned(), Value::Array(query));
    }
    if let Some(schema) = body {
        value.insert(
            "requestBody".to_owned(),
            json!({
                "required": false,
                "content": {"application/json": {"schema": schema}}
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
                        "schema": operation_schema(
                            &operation.response,
                            schemas,
                            names,
                            operation,
                            FieldSource::Response,
                            aliases,
                            false,
                        )
                    }
                }
            }
        }),
    );
    Ok(Value::Object(value))
}

fn operation_schema(
    type_ref: &TypeRef,
    schemas: &BTreeMap<String, Schema>,
    names: &BTreeMap<String, String>,
    operation: &Operation,
    source: FieldSource,
    aliases: &HashMap<String, String>,
    optional_fields: bool,
) -> Value {
    let fqn = type_ref.name.replace("::", ".");
    let Some(schema) = schemas.get(&fqn) else {
        return type_schema(type_ref, names, aliases);
    };
    if type_ref.array_depth == 0
        && !schema.type_parameters.is_empty()
        && schema.type_parameters.len() == type_ref.arguments.len()
    {
        return schema_object(
            schema,
            names,
            Some(operation),
            source,
            aliases,
            &schema.bindings_for(type_ref),
            optional_fields,
        );
    }
    type_schema(type_ref, names, aliases)
}

fn schema_object(
    schema: &Schema,
    names: &BTreeMap<String, String>,
    operation: Option<&Operation>,
    source: FieldSource,
    aliases: &HashMap<String, String>,
    bindings: &BTreeMap<String, TypeRef>,
    optional_fields: bool,
) -> Value {
    let patches = operation
        .map(|operation| {
            operation
                .semantic_patches
                .iter()
                .filter(|patch| {
                    patch.target.schema_fqn == schema.fqn && patch.target.source == source
                })
                .map(|patch| (patch.target.field_name.as_str(), patch))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in &schema.fields {
        let mut property = type_schema(&field.java_type.substitute(bindings), names, aliases);
        if let Some(description) = &field.description {
            property
                .as_object_mut()
                .expect("schema object")
                .insert("description".to_owned(), json!(description));
        }
        let associated_patch = patches
            .get(field.name.as_str())
            .filter(|patch| patch.associated_values().is_some());
        if let Some(patch) = associated_patch {
            apply_enum(
                &mut property,
                patch.associated_values().expect("associated enum values"),
            );
        }
        if patches
            .get(field.name.as_str())
            .is_some_and(|patch| patch.has_enum_null_branch())
        {
            apply_nullable(&mut property);
        }
        if let Some(patch) = associated_patch {
            let mut association = Map::new();
            association.insert("enumAssociated".to_owned(), json!(true));
            if let Some(enum_fqn) = &patch.enum_fqn {
                association.insert("enumFqn".to_owned(), json!(enum_fqn));
            }
            if let Some(enum_source) = &patch.enum_source {
                association.insert("enumSource".to_owned(), json!(enum_source));
            }
            if let Some(accessor) = &patch.accessor {
                association.insert("accessor".to_owned(), json!(accessor));
            }
            property.as_object_mut().expect("schema object").insert(
                "x-nlab-enum-association".to_owned(),
                Value::Object(association),
            );
        }
        properties.insert(field.name.clone(), property);
        if !optional_fields && !field.optional {
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

fn apply_nullable(schema: &mut Value) {
    let description = schema
        .as_object_mut()
        .and_then(|value| value.remove("description"));
    let original = std::mem::take(schema);
    *schema = json!({ "anyOf": [original, { "type": "null" }] });
    if let Some(description) = description {
        schema["description"] = description;
    }
}

fn semantic_patch(patch: &SemanticPatch) -> Value {
    let mut value = Map::new();
    value.insert("target".to_owned(), json!(patch.target));
    value.insert("status".to_owned(), json!(patch.status));
    if patch.enum_associated {
        value.insert("enumAssociated".to_owned(), json!(true));
    }
    if patch.primary_enum_value {
        value.insert("primaryEnumValue".to_owned(), json!(true));
    }
    if let Some(candidate) = &patch.enum_candidate {
        value.insert("enumCandidate".to_owned(), json!(candidate));
    }
    if patch.nullable {
        value.insert("nullable".to_owned(), json!(true));
    }
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
    if !patch.known_values.is_empty() {
        value.insert("knownValues".to_owned(), json!(patch.known_values));
    }
    if !patch.evidence.is_empty() {
        value.insert("evidence".to_owned(), json!(patch.evidence));
    }
    if let Some(warning) = &patch.warning {
        value.insert("warning".to_owned(), json!(warning));
    }
    Value::Object(value)
}

pub(crate) struct SchemaVariant {
    pub fqn: String,
    pub name: String,
    pub operation: Option<usize>,
    pub source: FieldSource,
    pub optional_fields: bool,
    pub usages: BTreeSet<usize>,
}

pub(crate) struct SchemaPlan {
    pub variants: Vec<SchemaVariant>,
    pub aliases: HashMap<String, HashMap<String, String>>,
}

struct SchemaUse {
    fqn: String,
    operation: Option<usize>,
    source: FieldSource,
    optional_fields: bool,
    context: String,
}

pub(crate) fn enum_identity(patch: &SemanticPatch) -> String {
    format!(
        "{}{}#{}",
        if patch.enum_fqn.is_none() {
            "comment:"
        } else {
            ""
        },
        patch
            .enum_fqn
            .as_deref()
            .unwrap_or(&patch.target.schema_fqn),
        patch
            .accessor
            .as_deref()
            .unwrap_or(&patch.target.field_name)
    )
}

pub(crate) fn schema_plan(ir: &ContractIr) -> SchemaPlan {
    let names = schema_names(&ir.schemas);
    let requests = request_schemas(ir);
    // Original schemas let shared consumers keep a general type when domains differ.
    let mut uses = ir
        .schemas
        .keys()
        .map(|fqn| SchemaUse {
            fqn: fqn.clone(),
            operation: None,
            source: FieldSource::Response,
            optional_fields: requests.contains(fqn),
            context: String::new(),
        })
        .collect::<Vec<_>>();
    let mut operations = ir.operations.iter().enumerate().collect::<Vec<_>>();
    operations.sort_by_key(|(_, operation)| &operation.key);
    for (index, operation) in operations {
        for source in [FieldSource::Request, FieldSource::Response] {
            for root in source.roots(operation) {
                for fqn in reachable_schemas(root, &ir.schemas) {
                    uses.push(SchemaUse {
                        optional_fields: source == FieldSource::Request
                            && root.name.replace("::", ".") == fqn,
                        fqn,
                        operation: Some(index),
                        source,
                        context: source.operation_key(operation),
                    });
                }
            }
        }
    }
    let original_groups = ir
        .schemas
        .keys()
        .enumerate()
        .map(|(i, fqn)| (fqn, i))
        .collect::<BTreeMap<_, _>>();
    let mut groups = uses
        .iter()
        .map(|usage| original_groups[&usage.fqn])
        .collect::<Vec<_>>();
    // ponytail: whole-graph refinement; use a worklist if large contract graphs make this slow.
    // Including referenced groups propagates nested differences and handles recursive schemas.
    loop {
        let mut aliases = HashMap::<String, HashMap<String, String>>::new();
        for (usage, group) in uses.iter().zip(&groups) {
            aliases
                .entry(usage.context.clone())
                .or_default()
                .insert(usage.fqn.clone(), group.to_string());
        }
        let mut signatures = BTreeMap::new();
        let next =
            uses.iter()
                .zip(&groups)
                .map(|(usage, group)| {
                    let schema = &ir.schemas[&usage.fqn];
                    let mut shape = schema_object(
                        schema,
                        &names,
                        usage.operation.map(|index| &ir.operations[index]),
                        usage.source,
                        &aliases[&usage.context],
                        &BTreeMap::new(),
                        usage.optional_fields,
                    );
                    // OpenAPI references omit generic arguments; TypeScript keeps them.
                    let mut references = Vec::new();
                    for field in &schema.fields {
                        let patch =
                            usage
                                .operation
                                .and_then(|index| {
                                    ir.operations[index].semantic_patches.iter().rev().find(
                                        |patch| {
                                            patch.target.schema_fqn == schema.fqn
                                                && patch.target.source == usage.source
                                                && patch.target.field_name == field.name
                                        },
                                    )
                                })
                                .filter(|patch| patch.associated_values().is_some());
                        if let Some(patch) = patch {
                            // Evidence paths do not change the emitted enum type.
                            shape["properties"][&field.name]["x-nlab-enum-association"] =
                                json!(enum_identity(patch));
                        } else {
                            referenced_groups(
                                &field.java_type,
                                &aliases[&usage.context],
                                &mut references,
                            );
                        }
                    }
                    let next_id = signatures.len();
                    *signatures
                        .entry((*group, shape.to_string(), references))
                        .or_insert(next_id)
                })
                .collect::<Vec<_>>();
        if next == groups {
            break;
        }
        groups = next;
    }

    let mut members = BTreeMap::<usize, Vec<&SchemaUse>>::new();
    let mut used_groups = BTreeMap::<String, BTreeSet<usize>>::new();
    for (usage, group) in uses.iter().zip(&groups) {
        members.entry(*group).or_default().push(usage);
        if usage.operation.is_some() {
            used_groups
                .entry(usage.fqn.clone())
                .or_default()
                .insert(*group);
        }
    }
    // Shared consumers need a general type across differing domains, including its dependencies.
    let shared = used_groups
        .iter()
        .filter(|(_, schema_groups)| schema_groups.len() > 1)
        .flat_map(|(fqn, _)| {
            reachable_schemas(
                &TypeRef {
                    name: fqn.clone(),
                    arguments: Vec::new(),
                    array_depth: 0,
                },
                &ir.schemas,
            )
        })
        .collect::<BTreeSet<_>>();
    for fqn in shared {
        used_groups
            .entry(fqn.clone())
            .or_default()
            .insert(groups[original_groups[&fqn]]);
    }
    let mut group_names = BTreeMap::new();
    let mut seeds = BTreeMap::new();
    for (fqn, schema_groups) in &used_groups {
        for group in schema_groups {
            if schema_groups.len() == 1
                || members[group].iter().any(|usage| usage.operation.is_none())
            {
                group_names.insert(*group, names[fqn].clone());
                continue;
            }
            let usage = members[group][0];
            let operation = &ir.operations[usage.operation.expect("used schema")];
            let mut seed = vec![
                without_interface_prefix(&operation.facade_name).to_owned(),
                operation.method_name.clone(),
            ];
            if usage.source == FieldSource::Request {
                seed.push("Request".to_owned());
            }
            seed.push(ir.schemas[fqn].name.clone());
            seeds.insert(group.to_string(), seed);
        }
    }
    let reserved = names.values().cloned().collect::<BTreeSet<_>>();
    let alias_names = shortest_unique_names_avoiding(&seeds, &reserved);
    for (group, name) in alias_names {
        group_names.insert(group.parse::<usize>().expect("schema group"), name);
    }
    let mut aliases = HashMap::<String, HashMap<String, String>>::new();
    for (usage, group) in uses
        .iter()
        .zip(&groups)
        .filter(|(usage, _)| usage.operation.is_some())
    {
        aliases
            .entry(usage.context.clone())
            .or_default()
            .insert(usage.fqn.clone(), group_names[group].clone());
    }
    let mut variants = Vec::new();
    for (group, name) in group_names {
        let usage = members[&group]
            .iter()
            .find(|usage| usage.operation.is_some())
            .unwrap_or(&members[&group][0]);
        let usages = if usage.operation.is_some() {
            members[&group].clone()
        } else {
            uses.iter().filter(|other| other.fqn == usage.fqn).collect()
        };
        variants.push(SchemaVariant {
            fqn: usage.fqn.clone(),
            name,
            operation: usage.operation,
            source: usage.source,
            optional_fields: usage.optional_fields,
            usages: usages.iter().filter_map(|usage| usage.operation).collect(),
        });
    }
    variants.sort_by(|left, right| left.name.cmp(&right.name));
    SchemaPlan { variants, aliases }
}

fn referenced_groups(value: &TypeRef, aliases: &HashMap<String, String>, groups: &mut Vec<String>) {
    if let Some(group) = aliases.get(&value.name.replace("::", ".")) {
        groups.push(group.clone());
    }
    for argument in &value.arguments {
        referenced_groups(argument, aliases, groups);
    }
}

pub(crate) fn request_schemas(ir: &ContractIr) -> BTreeSet<String> {
    ir.operations
        .iter()
        .flat_map(|operation| FieldSource::Request.roots(operation))
        .map(|request| request.name.replace("::", "."))
        .filter(|fqn| ir.schemas.contains_key(fqn))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nullable_enum_schema_accepts_null_instances() {
        let mut schema = json!({"type": "string"});
        apply_enum(
            &mut schema,
            &[
                CodedValue {
                    value: WireValue::String("a".to_owned()),
                    key: None,
                    label: "A".to_owned(),
                },
                CodedValue {
                    value: WireValue::String("b".to_owned()),
                    key: None,
                    label: "B".to_owned(),
                },
            ],
        );
        apply_nullable(&mut schema);

        let validator = jsonschema::options().build(&schema).unwrap();
        assert!(validator.is_valid(&Value::Null));
        assert!(validator.is_valid(&json!("a")));
        assert!(!validator.is_valid(&json!("c")));
        let enum_schema = &schema["anyOf"][0];
        assert_eq!(enum_schema["enum"], json!(["a", "b"]));
        assert_eq!(enum_schema["x-enum-varnames"].as_array().unwrap().len(), 2);
        assert_eq!(
            enum_schema["x-enum-descriptions"].as_array().unwrap().len(),
            2
        );
        assert_eq!(schema["anyOf"][1], json!({"type": "null"}));
    }
}
