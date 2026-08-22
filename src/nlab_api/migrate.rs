use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use ignore::WalkBuilder;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use super::naming::upper_camel;

#[derive(Clone, Debug, Args)]
pub struct MigrateArgs {
    /// New generated artifact directory
    #[arg(long, value_name = "path")]
    project: PathBuf,
    /// Previous generated artifact directory
    #[arg(long, value_name = "path")]
    legacy: PathBuf,
    /// Business source root whose relative imports should be migrated
    #[arg(long, value_name = "path")]
    source_root: Option<PathBuf>,
    /// Apply unique path-only migrations; default only reports
    #[arg(long)]
    apply: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplacementMap {
    version: u8,
    old_openapi_sha256: String,
    new_openapi_sha256: String,
    interfaces: Vec<InterfaceReplacement>,
    types: Vec<TypeReplacement>,
    enum_members: Vec<EnumMemberReplacement>,
    unresolved: Vec<String>,
    changed_source_files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct InterfaceReplacement {
    operation_key: String,
    old_file: String,
    new_file: String,
    old_export: String,
    new_export: String,
    status: ReplacementStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeReplacement {
    old_file: String,
    new_file: String,
    old_export: String,
    new_export: String,
    status: ReplacementStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ReplacementStatus {
    Unchanged,
    Moved,
    Renamed,
    MovedAndRenamed,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnumMemberReplacement {
    target: String,
    wire_value: String,
    old_member: String,
    new_member: String,
}

pub fn run(args: MigrateArgs) -> u8 {
    match run_inner(args) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("serialize migrate result")
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

pub(crate) fn snapshot_legacy(project: &Path) -> Result<Option<tempfile::TempDir>> {
    let manifest_path = project.join(".nlab/frontend-manifest.json");
    let stable_openapi = project.join(".nlab/openapi.json");
    if !manifest_path.is_file() || !stable_openapi.is_file() {
        return Ok(None);
    }
    let manifest = read_manifest(project)?;
    let snapshot = tempfile::tempdir().context("create legacy generation snapshot")?;
    fs::create_dir_all(snapshot.path().join(".nlab"))?;
    fs::copy(
        &manifest_path,
        snapshot.path().join(".nlab/frontend-manifest.json"),
    )?;
    fs::copy(&stable_openapi, snapshot.path().join(".nlab/openapi.json"))?;
    for relative in manifest
        .api_files
        .iter()
        .chain(&manifest.type_files)
        .chain(&manifest.enum_files)
    {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("unsafe legacy generated path: {relative}");
        }
        let source = project.join(relative_path);
        if !source.is_file() {
            continue;
        }
        let target = snapshot.path().join(relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, target)?;
    }
    Ok(Some(snapshot))
}

pub(crate) fn automatic(project: &Path, legacy: &Path, source_root: &Path) -> Result<Value> {
    run_inner(MigrateArgs {
        project: project.to_owned(),
        legacy: legacy.to_owned(),
        source_root: Some(source_root.to_owned()),
        apply: true,
    })
}

fn run_inner(args: MigrateArgs) -> Result<Value> {
    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("resolve new generated project {}", args.project.display()))?;
    let legacy = args
        .legacy
        .canonicalize()
        .with_context(|| format!("resolve legacy generated project {}", args.legacy.display()))?;
    if project == legacy {
        bail!("--project and --legacy must be different directories");
    }
    let new_manifest = read_manifest(&project)?;
    let old_manifest = read_manifest(&legacy)?;
    let config = super::config::ProjectConfig::load(&project)?;
    let new_openapi = read_openapi(&project)?;
    let old_openapi = read_openapi(&legacy)?;
    let new_root = common_root(&new_manifest.api_files).context("new API root is ambiguous")?;
    let old_root = common_root(&old_manifest.api_files).context("legacy API root is ambiguous")?;
    let new_operations = operations(&new_openapi, &new_root);
    let old_operations = operations(&old_openapi, &old_root);
    let mut unresolved = Vec::new();
    let mut interfaces = Vec::new();
    for (key, old) in &old_operations {
        let Some(new) = new_operations.get(key) else {
            interfaces.push(InterfaceReplacement {
                operation_key: key.clone(),
                old_file: old.file.clone(),
                new_file: String::new(),
                old_export: old.export.clone(),
                new_export: String::new(),
                status: ReplacementStatus::Removed,
            });
            continue;
        };
        let moved = old.file != new.file;
        let renamed = old.export != new.export;
        interfaces.push(InterfaceReplacement {
            operation_key: key.clone(),
            old_file: old.file.clone(),
            new_file: new.file.clone(),
            old_export: old.export.clone(),
            new_export: new.export.clone(),
            status: match (moved, renamed) {
                (false, false) => ReplacementStatus::Unchanged,
                (true, false) => ReplacementStatus::Moved,
                (false, true) => ReplacementStatus::Renamed,
                (true, true) => ReplacementStatus::MovedAndRenamed,
            },
        });
    }
    interfaces.sort_by(|left, right| left.operation_key.cmp(&right.operation_key));
    let old_type_files = old_manifest
        .type_files
        .iter()
        .chain(&old_manifest.enum_files)
        .cloned()
        .collect::<Vec<_>>();
    let new_type_files = new_manifest
        .type_files
        .iter()
        .chain(&new_manifest.enum_files)
        .cloned()
        .collect::<Vec<_>>();
    let inferred_names = inferred_type_names(
        &old_openapi,
        &new_openapi,
        &legacy,
        &project,
        &interfaces,
        &old_type_files,
        &new_type_files,
    )?;
    let types = type_replacements(
        &legacy,
        &old_type_files,
        &project,
        &new_type_files,
        &inferred_names,
    )?;
    let mut enum_members = enum_replacements(&old_openapi, &new_openapi);
    enum_members.extend(generated_enum_replacements(
        &legacy,
        &old_type_files,
        &project,
        &new_type_files,
        &inferred_names,
    )?);
    enum_members.sort_by(|left, right| {
        (&left.target, &left.wire_value).cmp(&(&right.target, &right.wire_value))
    });
    enum_members
        .dedup_by(|left, right| left.target == right.target && left.wire_value == right.wire_value);
    let mut changed_source_files = Vec::new();
    if let Some(source_root) = args.source_root {
        let source_root = source_root
            .canonicalize()
            .with_context(|| format!("resolve business source root {}", source_root.display()))?;
        changed_source_files = migrate_relative_imports(SourceMigration {
            source_root: &source_root,
            project_root: &project,
            interfaces: &interfaces,
            types: &types,
            enum_members: &enum_members,
            source_directory: &config.frontend.source_root,
            unresolved: &mut unresolved,
            apply: args.apply,
        })?;
    }
    unresolved.sort();
    unresolved.dedup();
    let map = ReplacementMap {
        version: 1,
        old_openapi_sha256: old_manifest.openapi_sha256,
        new_openapi_sha256: new_manifest.openapi_sha256,
        interfaces,
        types,
        enum_members,
        unresolved,
        changed_source_files,
    };
    let map_path = project.join(".nlab/replacement-map.json");
    atomic_write(
        &map_path,
        &format!("{}\n", serde_json::to_string_pretty(&map)?),
    )?;
    Ok(serde_json::json!({
        "status": if map.unresolved.is_empty() { "complete" } else { "decision-required" },
        "replacementMap": map_path,
        "interfaces": map.interfaces.len(),
        "types": map.types.len(),
        "enumMembers": map.enum_members.len(),
        "unresolvedCount": map.unresolved.len(),
        "changedSourceFiles": map.changed_source_files,
        "applied": args.apply
    }))
}

#[derive(Clone, Debug)]
struct OperationShape {
    file: String,
    export: String,
}

fn operations(document: &Value, api_root: &str) -> BTreeMap<String, OperationShape> {
    document["x-nlab-contracts"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, operation)| {
            let file = operation["x-nlab-api-output"].as_str()?;
            let export = operation["x-nlab-method-name"]
                .as_str()
                .or_else(|| operation["operationId"].as_str())?;
            let file = if file == api_root || file.starts_with(&format!("{api_root}/")) {
                file.to_owned()
            } else {
                join_path(api_root, file)
            };
            Some((
                key.clone(),
                OperationShape {
                    file,
                    export: export.to_owned(),
                },
            ))
        })
        .collect()
}

fn inferred_type_names(
    old_openapi: &Value,
    new_openapi: &Value,
    old_root: &Path,
    new_root: &Path,
    interfaces: &[InterfaceReplacement],
    old_type_files: &[String],
    new_type_files: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut inferred = schema_type_names(old_openapi, new_openapi);
    for replacement in interfaces
        .iter()
        .filter(|replacement| replacement.status != ReplacementStatus::Removed)
    {
        let old_source = fs::read_to_string(old_root.join(&replacement.old_file))?;
        let new_source = fs::read_to_string(new_root.join(&replacement.new_file))?;
        let old_types = function_types(&old_source, &replacement.old_export);
        let new_types = function_types(&new_source, &replacement.new_export);
        if let (Some(old), Some(new)) = (old_types.request, new_types.request) {
            inferred.entry(old).or_insert(new);
        }
        if let (Some(old), Some(new)) = (old_types.response, new_types.response) {
            inferred.entry(old).or_insert(new);
        }
    }
    inferred.extend(enum_type_names(
        old_root,
        old_type_files,
        new_root,
        new_type_files,
    )?);
    inferred.extend(linked_field_enum_type_names(
        old_openapi,
        new_openapi,
        new_root,
        new_type_files,
        &inferred,
    )?);
    Ok(inferred)
}

fn linked_field_enum_type_names(
    old_openapi: &Value,
    new_openapi: &Value,
    new_root: &Path,
    new_files: &[String],
    inferred_schemas: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let new_enums = enum_exports(new_root, new_files)?;
    let mut inferred = BTreeMap::new();
    for (old_schema_name, new_schema_name) in inferred_schemas {
        let Some(old_properties) = old_openapi
            .pointer(&format!("/components/schemas/{old_schema_name}/properties"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        let Some(new_properties) = new_openapi
            .pointer(&format!("/components/schemas/{new_schema_name}/properties"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (field, old_property) in old_properties {
            if old_property.get("enum").and_then(Value::as_array).is_none() {
                continue;
            }
            let Some(enum_fqn) = new_properties
                .get(field)
                .and_then(|property| property.pointer("/x-nlab-linked-enum/enumFqn"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let enum_name = enum_fqn
                .rsplit('.')
                .next()
                .unwrap_or(enum_fqn)
                .trim_end_matches("Enum");
            let mut candidates = new_enums
                .keys()
                .map(|candidate| (enum_name_score(enum_name, candidate), candidate))
                .filter(|(score, _)| *score > 0)
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| right.cmp(left));
            if let Some((best_score, best_name)) = candidates.first()
                && candidates
                    .get(1)
                    .is_none_or(|(score, _)| score < best_score)
            {
                inferred.insert(
                    format!("{old_schema_name}{}", upper_camel(field)),
                    (*best_name).clone(),
                );
            }
        }
    }
    Ok(inferred)
}

fn enum_type_names(
    old_root: &Path,
    old_files: &[String],
    new_root: &Path,
    new_files: &[String],
) -> Result<BTreeMap<String, String>> {
    let old = enum_exports(old_root, old_files)?;
    let new = enum_exports(new_root, new_files)?;
    let mut inferred = BTreeMap::new();
    for (old_name, old_values) in old {
        let mut candidates = new
            .iter()
            .map(|(name, values)| {
                let exact_values = values.keys().eq(old_values.keys());
                ((exact_values, enum_name_score(&old_name, name)), name)
            })
            .filter(|((exact_values, score), _)| (*exact_values && *score > 0) || *score >= 2)
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| right.cmp(left));
        if let Some((best_rank, best_name)) = candidates.first()
            && candidates.get(1).is_none_or(|(rank, _)| rank < best_rank)
        {
            inferred.insert(old_name, (*best_name).clone());
        }
    }
    Ok(inferred)
}

fn enum_exports(
    root: &Path,
    files: &[String],
) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let declaration = Regex::new(
        r"(?s)export\s+const\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*\{(?P<body>.*?)\}\s+as\s+const",
    )
    .expect("const enum declaration regex");
    let value = Regex::new(
        r#"(?m)^\s*(?P<member>[A-Za-z_$][A-Za-z0-9_$]*)\s*:\s*(?P<value>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|-?\d+),"#,
    )
    .expect("const enum value regex");
    let mut exports = BTreeMap::new();
    for relative in files {
        let path = root.join(relative);
        let Some(source) = read_optional_generated_source(&path, "read generated enum")? else {
            continue;
        };
        for captures in declaration.captures_iter(&source) {
            let Some(name) = captures.get(1) else {
                continue;
            };
            let values = value
                .captures_iter(captures.name("body").expect("enum body").as_str())
                .filter_map(|captures| {
                    Some((
                        ts_literal_key(captures.name("value")?.as_str()),
                        captures.name("member")?.as_str().to_owned(),
                    ))
                })
                .collect::<BTreeMap<_, _>>();
            if !values.is_empty() {
                exports.insert(name.as_str().to_owned(), values);
            }
        }
    }
    Ok(exports)
}

fn read_optional_generated_source(path: &Path, action: &str) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(Some(source)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("{action} {}", path.display())),
    }
}

fn ts_literal_key(value: &str) -> String {
    if value.len() >= 2 && matches!(value.as_bytes().first(), Some(b'\'' | b'"')) {
        format!("s:{}", &value[1..value.len() - 1])
    } else {
        format!("n:{value}")
    }
}

fn generated_enum_replacements(
    old_root: &Path,
    old_files: &[String],
    new_root: &Path,
    new_files: &[String],
    inferred_names: &BTreeMap<String, String>,
) -> Result<Vec<EnumMemberReplacement>> {
    let old = enum_exports(old_root, old_files)?;
    let new = enum_exports(new_root, new_files)?;
    let mut replacements = Vec::new();
    for (old_name, new_name) in inferred_names {
        let (Some(old_members), Some(new_members)) = (old.get(old_name), new.get(new_name)) else {
            continue;
        };
        for (wire, old_member) in old_members {
            if let Some(new_member) = new_members.get(wire) {
                replacements.push(EnumMemberReplacement {
                    target: old_name.clone(),
                    wire_value: wire.clone(),
                    old_member: old_member.clone(),
                    new_member: new_member.clone(),
                });
            }
        }
    }
    Ok(replacements)
}

fn enum_name_score(left: &str, right: &str) -> usize {
    let left = name_words(left);
    let right = name_words(right);
    left.intersection(&right).count()
}

fn name_words(value: &str) -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    let mut current = String::new();
    for character in value.chars() {
        if character.is_ascii_uppercase() && !current.is_empty() {
            insert_name_word(&mut words, &current);
            current.clear();
        }
        if character.is_ascii_alphanumeric() {
            current.push(character);
        }
    }
    if !current.is_empty() {
        insert_name_word(&mut words, &current);
    }
    words
}

fn insert_name_word(words: &mut BTreeSet<String>, value: &str) {
    let value = value.to_ascii_lowercase();
    if value.len() > 1
        && !matches!(
            value.as_str(),
            "type" | "code" | "status" | "enum" | "request" | "response" | "req" | "resp"
        )
    {
        words.insert(value);
    }
}

fn schema_type_names(old: &Value, new: &Value) -> BTreeMap<String, String> {
    let mut inferred = BTreeMap::new();
    let new_schemas = new["components"]["schemas"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let new_base_by_simple = new_schemas
        .iter()
        .filter_map(|(name, schema)| {
            let fqn = schema["x-nlab-schema-fqn"].as_str()?;
            let simple = fqn.rsplit(['.', ':']).find(|part| !part.is_empty())?;
            (name == simple).then(|| (simple.to_owned(), name.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    for (old_name, schema) in old["components"]["schemas"]
        .as_object()
        .into_iter()
        .flatten()
    {
        let Some(java_type) = schema["x-nlab-java-type"].as_str() else {
            continue;
        };
        let simple = java_type
            .split('<')
            .next()
            .unwrap_or(java_type)
            .rsplit(['.', ':'])
            .find(|part| !part.is_empty())
            .unwrap_or(java_type);
        if let Some(new_name) = new_base_by_simple.get(simple) {
            inferred.insert(old_name.clone(), new_name.clone());
        }
    }

    let old_operations = old["x-nlab-contracts"].as_object();
    let new_operations = new["x-nlab-contracts"].as_object();
    if let (Some(old_operations), Some(new_operations)) = (old_operations, new_operations) {
        let mut visiting = BTreeSet::new();
        for (key, old_operation) in old_operations {
            let Some(new_operation) = new_operations.get(key) else {
                continue;
            };
            for (old_schema, new_schema) in [
                (request_schema(old_operation), request_schema(new_operation)),
                (
                    response_schema(old_operation),
                    response_schema(new_operation),
                ),
            ] {
                if let (Some(old_schema), Some(new_schema)) = (old_schema, new_schema) {
                    collect_schema_pairs(
                        old_schema,
                        new_schema,
                        old,
                        new,
                        &mut inferred,
                        &mut visiting,
                    );
                }
            }
        }
    }
    inferred
}

fn request_schema(operation: &Value) -> Option<&Value> {
    operation.pointer("/requestBody/content/application~1json/schema")
}

fn response_schema(operation: &Value) -> Option<&Value> {
    operation["responses"]
        .as_object()?
        .iter()
        .filter(|(status, _)| status.starts_with('2'))
        .find_map(|(_, response)| response.pointer("/content/application~1json/schema"))
}

fn collect_schema_pairs(
    old_schema: &Value,
    new_schema: &Value,
    old_document: &Value,
    new_document: &Value,
    inferred: &mut BTreeMap<String, String>,
    visiting: &mut BTreeSet<(String, String)>,
) {
    let old_name = reference_name(old_schema);
    let new_name = reference_name(new_schema);
    if let (Some(old_name), Some(new_name)) = (&old_name, &new_name) {
        inferred.entry(old_name.clone()).or_insert(new_name.clone());
        if !visiting.insert((old_name.clone(), new_name.clone())) {
            return;
        }
    }
    let old_schema = old_name
        .as_deref()
        .and_then(|name| old_document.pointer(&format!("/components/schemas/{name}")))
        .unwrap_or(old_schema);
    let new_schema = new_name
        .as_deref()
        .and_then(|name| new_document.pointer(&format!("/components/schemas/{name}")))
        .unwrap_or(new_schema);
    if let (Some(old_properties), Some(new_properties)) = (
        old_schema.get("properties").and_then(Value::as_object),
        new_schema.get("properties").and_then(Value::as_object),
    ) {
        for (name, old_property) in old_properties {
            if let Some(new_property) = new_properties.get(name) {
                collect_schema_pairs(
                    old_property,
                    new_property,
                    old_document,
                    new_document,
                    inferred,
                    visiting,
                );
            }
        }
    }
    if let (Some(old_items), Some(new_items)) = (old_schema.get("items"), new_schema.get("items")) {
        collect_schema_pairs(
            old_items,
            new_items,
            old_document,
            new_document,
            inferred,
            visiting,
        );
    }
}

fn reference_name(schema: &Value) -> Option<String> {
    schema
        .get("$ref")?
        .as_str()?
        .strip_prefix("#/components/schemas/")
        .map(ToOwned::to_owned)
}

#[derive(Default)]
struct FunctionTypes {
    request: Option<String>,
    response: Option<String>,
}

fn function_types(source: &str, name: &str) -> FunctionTypes {
    let marker = format!("function {name}");
    let Some(start) = source.find(&marker) else {
        return FunctionTypes::default();
    };
    let tail = &source[start + marker.len()..];
    let Some(open) = tail.find('(') else {
        return FunctionTypes::default();
    };
    let Some(close) = matching_delimiter(tail, open, b'(', b')') else {
        return FunctionTypes::default();
    };
    let parameters = &tail[open + 1..close];
    let request = parameters
        .split(',')
        .map(str::trim)
        .find(|parameter| !parameter.is_empty() && !parameter.starts_with("options"))
        .and_then(|parameter| parameter.split_once(':').map(|(_, value)| value.trim()))
        .and_then(first_type_identifier);
    let body = &tail[close + 1..];
    let body = &body[..body.find("\nexport ").unwrap_or(body.len())];
    let response = Regex::new(r"(?:Promise|nlabRequest)<([A-Za-z_$][A-Za-z0-9_$]*)>")
        .expect("function response regex")
        .captures(body)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    FunctionTypes { request, response }
}

fn matching_delimiter(source: &str, open: usize, left: u8, right: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, byte) in source.as_bytes().iter().copied().enumerate().skip(open) {
        if byte == left {
            depth += 1;
        } else if byte == right {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn first_type_identifier(value: &str) -> Option<String> {
    Regex::new(r"[A-Za-z_$][A-Za-z0-9_$]*")
        .expect("type identifier regex")
        .find(value)
        .map(|value| value.as_str().to_owned())
}

fn type_replacements(
    old_root: &Path,
    old_files: &[String],
    new_root: &Path,
    new_files: &[String],
    inferred_names: &BTreeMap<String, String>,
) -> Result<Vec<TypeReplacement>> {
    let old_symbols = exported_symbols(old_root, old_files)?;
    let new_symbols = exported_symbols(new_root, new_files)?;
    let mut output = Vec::new();
    for (symbol, old_locations) in old_symbols {
        let new_export = if new_symbols.contains_key(&symbol) {
            symbol.clone()
        } else {
            inferred_names.get(&symbol).cloned().unwrap_or_default()
        };
        let new_locations = new_symbols
            .get(&new_export)
            .filter(|locations| locations.len() == 1);
        for old_file in old_locations {
            let Some(new_file) = new_locations
                .and_then(|locations| locations.first())
                .cloned()
            else {
                output.push(TypeReplacement {
                    old_file,
                    new_file: String::new(),
                    old_export: symbol.clone(),
                    new_export: String::new(),
                    status: ReplacementStatus::Removed,
                });
                continue;
            };
            let moved = old_file != new_file;
            let renamed = symbol != new_export;
            output.push(TypeReplacement {
                status: match (moved, renamed) {
                    (false, false) => ReplacementStatus::Unchanged,
                    (true, false) => ReplacementStatus::Moved,
                    (false, true) => ReplacementStatus::Renamed,
                    (true, true) => ReplacementStatus::MovedAndRenamed,
                },
                old_file,
                new_file,
                old_export: symbol.clone(),
                new_export: new_export.clone(),
            });
        }
    }
    output.sort_by(|left, right| {
        (&left.old_export, &left.old_file).cmp(&(&right.old_export, &right.old_file))
    });
    Ok(output)
}

fn exported_symbols(root: &Path, files: &[String]) -> Result<BTreeMap<String, Vec<String>>> {
    let pattern = Regex::new(
        r"(?m)^export\s+(?:declare\s+)?(?:interface|type|const|enum|class)\s+([A-Za-z_$][A-Za-z0-9_$]*)",
    )
    .expect("TypeScript export regex");
    let mut symbols = BTreeMap::<String, Vec<String>>::new();
    for relative in files {
        let path = root.join(relative);
        let Some(source) = read_optional_generated_source(&path, "read generated type file")?
        else {
            continue;
        };
        let mut file_symbols = pattern
            .captures_iter(&source)
            .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_owned()))
            .collect::<BTreeSet<_>>();
        for symbol in file_symbols.iter() {
            symbols
                .entry(symbol.clone())
                .or_default()
                .push(relative.clone());
        }
        file_symbols.clear();
    }
    for locations in symbols.values_mut() {
        locations.sort();
        locations.dedup();
    }
    Ok(symbols)
}

fn enum_replacements(old: &Value, new: &Value) -> Vec<EnumMemberReplacement> {
    let old = enum_targets(old);
    let new = enum_targets(new);
    let mut output = Vec::new();
    for (target, old_values) in old {
        let Some(new_values) = new.get(&target) else {
            continue;
        };
        for (wire, old_member) in old_values {
            if let Some(new_member) = new_values.get(&wire) {
                output.push(EnumMemberReplacement {
                    target: target.clone(),
                    wire_value: wire,
                    old_member,
                    new_member: new_member.clone(),
                });
            }
        }
    }
    output.sort_by(|left, right| {
        (&left.target, &left.wire_value).cmp(&(&right.target, &right.wire_value))
    });
    output
}

fn enum_targets(document: &Value) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut output = BTreeMap::new();
    for operation in document["x-nlab-contracts"]
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(_, operation)| {
            operation["x-nlab-semantic-patches"]
                .as_array()
                .into_iter()
                .flatten()
        })
    {
        let Some(target) = operation.get("target") else {
            continue;
        };
        let target_key = ["operationKey", "schemaFqn", "fieldPath"]
            .into_iter()
            .filter_map(|key| target[key].as_str())
            .collect::<Vec<_>>()
            .join("|");
        if target_key.matches('|').count() != 2 {
            continue;
        }
        let values = operation
            .get("values")
            .and_then(Value::as_array)
            .or_else(|| {
                operation
                    .pointer("/codedValues/values")
                    .and_then(Value::as_array)
            });
        let Some(values) = values else {
            continue;
        };
        let mapping = output.entry(target_key).or_insert_with(BTreeMap::new);
        for value in values {
            let wire = wire_key(&value["value"]);
            let member = value["key"].as_str().unwrap_or("");
            if !wire.is_empty() && !member.is_empty() {
                mapping.insert(wire, member.to_owned());
            }
        }
    }
    output
}

struct SourceMigration<'a> {
    source_root: &'a Path,
    project_root: &'a Path,
    interfaces: &'a [InterfaceReplacement],
    types: &'a [TypeReplacement],
    enum_members: &'a [EnumMemberReplacement],
    source_directory: &'a str,
    unresolved: &'a mut Vec<String>,
    apply: bool,
}

fn migrate_relative_imports(migration: SourceMigration<'_>) -> Result<Vec<String>> {
    let SourceMigration {
        source_root,
        project_root,
        interfaces,
        types,
        enum_members,
        source_directory,
        unresolved,
        apply,
    } = migration;
    let replacements = interfaces
        .iter()
        .map(|item| {
            (
                (
                    normalize_without_extension(&project_root.join(&item.old_file)),
                    item.old_export.clone(),
                ),
                SymbolTarget {
                    file: (!item.new_file.is_empty())
                        .then(|| normalize_without_extension(&project_root.join(&item.new_file))),
                    export: (!item.new_export.is_empty()).then(|| item.new_export.clone()),
                },
            )
        })
        .chain(types.iter().map(|item| {
            (
                (
                    normalize_without_extension(&project_root.join(&item.old_file)),
                    item.old_export.clone(),
                ),
                SymbolTarget {
                    file: (!item.new_file.is_empty())
                        .then(|| normalize_without_extension(&project_root.join(&item.new_file))),
                    export: (!item.new_export.is_empty()).then(|| item.new_export.clone()),
                },
            )
        }))
        .collect::<BTreeMap<_, _>>();
    let declaration_pattern = Regex::new(
        r#"(?ms)^(?P<indent>[ \t]*)(?P<keyword>(?:import|export)(?:\s+type)?)\s*\{(?P<names>[^}]*)\}\s+from\s+(?P<quote>['\"])(?P<path>(?:\.\.?/|@/)[^'\"]+)['\"](?P<semi>;?)"#,
    )
    .expect("static import/export regex");
    let mut changed = Vec::new();
    for entry in WalkBuilder::new(source_root)
        .hidden(false)
        .filter_entry(|entry| !is_ignored_directory(entry.path()))
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter(|entry| is_source_file(entry.path()))
    {
        let path = entry.path();
        let source = fs::read_to_string(path)
            .with_context(|| format!("read business source {}", path.display()))?;
        let mut changed_source = source.clone();
        let mut edits = Vec::new();
        for captures in declaration_pattern.captures_iter(&source) {
            let declaration = captures.get(0).expect("declaration capture");
            let specifier = captures.name("path").expect("module path capture").as_str();
            let old_file =
                resolve_module(path, source_root, project_root, source_directory, specifier);
            let mut groups = BTreeMap::<String, Vec<String>>::new();
            let mut declaration_changed = false;
            for imported in captures
                .name("names")
                .expect("import names capture")
                .as_str()
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                let imported = imported.strip_prefix("type ").unwrap_or(imported).trim();
                let (exported, local) = imported
                    .split_once(" as ")
                    .map(|(exported, local)| (exported.trim(), local.trim()))
                    .unwrap_or((imported, imported));
                let Some(target) = replacements.get(&(old_file.clone(), exported.to_owned()))
                else {
                    groups
                        .entry(old_file.clone())
                        .or_default()
                        .push(imported.to_owned());
                    continue;
                };
                let (Some(new_file), Some(new_export)) = (&target.file, &target.export) else {
                    unresolved.push(format!(
                        "removed generated export is still imported: {exported} ({specifier})"
                    ));
                    groups
                        .entry(old_file.clone())
                        .or_default()
                        .push(imported.to_owned());
                    continue;
                };
                declaration_changed |= new_file != &old_file || new_export != exported;
                let name = if new_export == local {
                    new_export.clone()
                } else {
                    format!("{new_export} as {local}")
                };
                groups.entry(new_file.clone()).or_default().push(name);
            }
            if !declaration_changed {
                continue;
            }
            let keyword = captures.name("keyword").expect("import keyword").as_str();
            let indent = captures.name("indent").expect("import indent").as_str();
            let quote = captures.name("quote").expect("import quote").as_str();
            let semi = captures.name("semi").expect("import semicolon").as_str();
            let declarations = groups
                .into_iter()
                .map(|(target, names)| {
                    let module = if specifier.starts_with("@/") {
                        alias_module(project_root, source_directory, &target)
                            .unwrap_or_else(|| relative_module(path, &target))
                    } else {
                        relative_module(path, &target)
                    };
                    format!(
                        "{indent}{keyword} {{ {} }} from {quote}{module}{quote}{semi}",
                        names.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            edits.push((declaration.range(), declarations));
        }
        for member in enum_members.iter().filter(|member| {
            is_typescript_identifier(&member.target) && member.old_member != member.new_member
        }) {
            let pattern = Regex::new(&format!(
                r"\b{}\s*\.\s*{}\b",
                regex::escape(&member.target),
                regex::escape(&member.old_member)
            ))?;
            for matched in pattern.find_iter(&source) {
                edits.push((
                    matched.range(),
                    format!("{}.{}", member.target, member.new_member),
                ));
            }
        }
        edits.sort_by_key(|(range, _)| range.start);
        edits.dedup_by(|left, right| left.0 == right.0);
        for (range, replacement) in edits.into_iter().rev() {
            changed_source.replace_range(range, &replacement);
        }
        if changed_source != source {
            changed.push(path.display().to_string());
            if apply {
                atomic_write(path, &changed_source)?;
            }
        }
    }
    changed.sort();
    Ok(changed)
}

fn is_typescript_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|character| {
        character == '_' || character == '$' || character.is_ascii_alphabetic()
    }) && characters
        .all(|character| character == '_' || character == '$' || character.is_ascii_alphanumeric())
}

#[derive(Clone, Debug)]
struct SymbolTarget {
    file: Option<String>,
    export: Option<String>,
}

fn resolve_module(
    source_file: &Path,
    source_root: &Path,
    project_root: &Path,
    source_directory: &str,
    specifier: &str,
) -> String {
    if let Some(relative) = specifier.strip_prefix("@/") {
        normalize_without_extension(&project_root.join(source_directory).join(relative))
    } else {
        normalize_without_extension(&source_file.parent().unwrap_or(source_root).join(specifier))
    }
}

fn alias_module(project_root: &Path, source_directory: &str, target: &str) -> Option<String> {
    let source_root = normalize_path(&project_root.join(source_directory));
    let relative = target.strip_prefix(&format!("{source_root}/"))?;
    Some(format!("@/{relative}"))
}

fn read_manifest(root: &Path) -> Result<super::model::FrontendManifest> {
    Ok(serde_json::from_str(&fs::read_to_string(
        root.join(".nlab/frontend-manifest.json"),
    )?)?)
}

fn read_openapi(root: &Path) -> Result<Value> {
    let pending = root.join(".nlab/openapi.pending.json");
    let path = if pending.is_file() {
        pending
    } else {
        root.join(".nlab/openapi.json")
    };
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn common_root(files: &[String]) -> Option<String> {
    let directories = files
        .iter()
        .map(|path| {
            path.rsplit_once('/')
                .map(|(directory, _)| directory)
                .unwrap_or("")
        })
        .collect::<Vec<_>>();
    let first = directories.first()?.split('/').collect::<Vec<_>>();
    let common = first
        .iter()
        .enumerate()
        .take_while(|(index, part)| {
            directories
                .iter()
                .skip(1)
                .all(|directory| directory.split('/').nth(*index) == Some(**part))
        })
        .map(|(_, part)| *part)
        .collect::<Vec<_>>()
        .join("/");
    (!common.is_empty()).then_some(common)
}

fn wire_key(value: &Value) -> String {
    match value {
        Value::String(value) => format!("s:{value}"),
        Value::Number(value) => format!("n:{value}"),
        _ => String::new(),
    }
}

fn atomic_write(path: &Path, source: &str) -> Result<()> {
    if fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!("refuse to replace symlink: {}", path.display());
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, source)
        .with_context(|| format!("write temporary file {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replace file {}", path.display()))
}

fn normalize_without_extension(path: &Path) -> String {
    normalize_path(&path.with_extension(""))
}

fn normalize_path(path: &Path) -> String {
    let mut values = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                values.pop();
            }
            Component::CurDir => {}
            Component::RootDir => values.clear(),
            Component::Normal(value) => values.push(value.to_string_lossy().into_owned()),
            Component::Prefix(value) => {
                values.push(value.as_os_str().to_string_lossy().into_owned())
            }
        }
    }
    format!("/{}", values.join("/"))
}

fn relative_module(from_file: &Path, target: &str) -> String {
    let from = normalize_path(from_file.parent().unwrap_or_else(|| Path::new("")));
    let from = from.trim_start_matches('/').split('/').collect::<Vec<_>>();
    let target = target
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(target[common..].iter().copied());
    let value = parts.join("/");
    if value.starts_with('.') {
        value
    } else {
        format!("./{value}")
    }
}

fn is_ignored_directory(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        matches!(
            name.to_str(),
            Some(".git" | ".nlab" | "node_modules" | "dist" | "coverage")
        )
    })
}

fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("ts" | "tsx" | "js" | "jsx" | "vue")
    )
}

fn join_path(left: &str, right: &str) -> String {
    format!(
        "{}/{}",
        left.trim_end_matches('/'),
        right.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_symbol_readers_skip_missing_manifest_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("status.ts"),
            "export const Status = {\n  STATUS_1: 1,\n} as const;\n",
        )
        .unwrap();

        let exports = enum_exports(
            root.path(),
            &["status.ts".to_owned(), "missing.ts".to_owned()],
        )
        .unwrap();
        let symbols = exported_symbols(
            root.path(),
            &["status.ts".to_owned(), "missing.ts".to_owned()],
        )
        .unwrap();

        assert_eq!(exports["Status"]["n:1"], "STATUS_1");
        assert_eq!(symbols["Status"], ["status.ts"]);
    }

    #[test]
    fn operation_mapping_detects_move_without_rename() {
        let old = serde_json::json!({
            "x-nlab-contracts": {
                "Facade#query": {
                    "x-nlab-api-output": "old/query.ts",
                    "x-nlab-method-name": "query"
                }
            }
        });
        let new = serde_json::json!({
            "x-nlab-contracts": {
                "Facade#query": {
                    "x-nlab-api-output": "new/query.ts",
                    "x-nlab-method-name": "query"
                }
            }
        });
        assert_ne!(
            operations(&old, "api")["Facade#query"].file,
            operations(&new, "api")["Facade#query"].file
        );
        assert_eq!(
            operations(&new, "src/service")["Facade#query"].file,
            "src/service/new/query.ts"
        );
        let rooted = serde_json::json!({
            "x-nlab-contracts": {
                "Facade#query": {
                    "x-nlab-api-output": "src/service/new/query.ts",
                    "x-nlab-method-name": "query"
                }
            }
        });
        assert_eq!(
            operations(&rooted, "src/service")["Facade#query"].file,
            "src/service/new/query.ts"
        );
        assert_eq!(
            common_root(&[
                "src/service/a.ts".to_owned(),
                "src/service/check/b.ts".to_owned(),
            ]),
            Some("src/service".to_owned())
        );
    }

    #[test]
    fn relative_module_keeps_import_inside_tree() {
        assert_eq!(
            relative_module(
                Path::new("/project/src/page/view.ts"),
                "/project/src/api/query"
            ),
            "../api/query"
        );
    }

    #[test]
    fn enum_name_matching_requires_business_words() {
        assert!(
            enum_name_score("TodayRecycleCountVOCode", "TodayCountTypeCode")
                > enum_name_score("BizButtonButtonType", "TodayCountTypeCode")
        );
        assert_eq!(
            enum_name_score("CreateRecycleOrderReqModelType", "TodayCountTypeCode"),
            0
        );
    }

    #[test]
    fn linked_field_enum_mapping_uses_java_enum_identity() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("reservationModelType.ts"),
            "export const ReservationModelType = { SPECIFIC: 1, UNKNOWN: 2, OTHER: 3, } as const;\n",
        )
        .unwrap();
        let old = serde_json::json!({
            "components": { "schemas": { "CreateReq": { "properties": {
                "modelType": { "type": "number", "enum": [1, 2, 3] }
            } } } }
        });
        let new = serde_json::json!({
            "components": { "schemas": { "CreateReq": { "properties": {
                "modelType": {
                    "type": "number",
                    "enum": [1, 2, 3],
                    "x-nlab-linked-enum": {
                        "enumFqn": "p.ReservationModelTypeEnum",
                        "accessor": "getType"
                    }
                }
            } } } }
        });
        let schemas = BTreeMap::from([("CreateReq".to_owned(), "CreateReq".to_owned())]);

        let inferred = linked_field_enum_type_names(
            &old,
            &new,
            root.path(),
            &["reservationModelType.ts".to_owned()],
            &schemas,
        )
        .unwrap();

        assert_eq!(
            inferred.get("CreateReqModelType").map(String::as_str),
            Some("ReservationModelType")
        );
    }

    #[test]
    fn migration_splits_barrel_exports_and_preserves_local_type_name() {
        let root = tempfile::tempdir().unwrap();
        let source_root = root.path().join("src");
        fs::create_dir_all(source_root.join("service")).unwrap();
        fs::create_dir_all(source_root.join("page")).unwrap();
        fs::write(
            source_root.join("service/index.ts"),
            "export { first, second } from './old'\n",
        )
        .unwrap();
        fs::write(
            source_root.join("page/view.ts"),
            "import type { OldType } from '@/types/old'\n",
        )
        .unwrap();
        fs::write(
            source_root.join("page/metric.ts"),
            "import { OldEnum } from '@/types/oldEnum'\nconst value = OldEnum.OLD_ONE\n",
        )
        .unwrap();
        let interfaces = vec![
            InterfaceReplacement {
                operation_key: "F#first".to_owned(),
                old_file: "src/service/old.ts".to_owned(),
                new_file: "src/service/a.ts".to_owned(),
                old_export: "first".to_owned(),
                new_export: "first".to_owned(),
                status: ReplacementStatus::Moved,
            },
            InterfaceReplacement {
                operation_key: "F#second".to_owned(),
                old_file: "src/service/old.ts".to_owned(),
                new_file: "src/service/b.ts".to_owned(),
                old_export: "second".to_owned(),
                new_export: "second".to_owned(),
                status: ReplacementStatus::Moved,
            },
        ];
        let types = vec![
            TypeReplacement {
                old_file: "src/types/old.ts".to_owned(),
                new_file: "src/types/new.ts".to_owned(),
                old_export: "OldType".to_owned(),
                new_export: "NewType".to_owned(),
                status: ReplacementStatus::MovedAndRenamed,
            },
            TypeReplacement {
                old_file: "src/types/oldEnum.ts".to_owned(),
                new_file: "src/types/newEnum.ts".to_owned(),
                old_export: "OldEnum".to_owned(),
                new_export: "NewEnum".to_owned(),
                status: ReplacementStatus::MovedAndRenamed,
            },
        ];
        let enum_members = vec![EnumMemberReplacement {
            target: "OldEnum".to_owned(),
            wire_value: "1".to_owned(),
            old_member: "OLD_ONE".to_owned(),
            new_member: "NEW_ONE".to_owned(),
        }];
        let mut unresolved = Vec::new();

        let changed = migrate_relative_imports(SourceMigration {
            source_root: &source_root,
            project_root: root.path(),
            interfaces: &interfaces,
            types: &types,
            enum_members: &enum_members,
            source_directory: "src",
            unresolved: &mut unresolved,
            apply: true,
        })
        .unwrap();

        assert_eq!(changed.len(), 3);
        assert!(unresolved.is_empty());
        assert_eq!(
            fs::read_to_string(source_root.join("service/index.ts")).unwrap(),
            "export { first } from './a'\nexport { second } from './b'\n"
        );
        assert_eq!(
            fs::read_to_string(source_root.join("page/view.ts")).unwrap(),
            "import type { NewType as OldType } from '@/types/new'\n"
        );
        assert_eq!(
            fs::read_to_string(source_root.join("page/metric.ts")).unwrap(),
            "import { NewEnum as OldEnum } from '@/types/newEnum'\nconst value = OldEnum.NEW_ONE\n"
        );
    }
}
