use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Args)]
pub struct MockArgs {
    /// Generated artifact directory containing OpenAPI
    #[arg(long, value_name = "path")]
    project: PathBuf,
    /// Mock output root inside project
    #[arg(long, default_value = "mock")]
    output_root: String,
    /// Stable global seed
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Calculate output without writing files
    #[arg(long)]
    dry_run: bool,
    /// Adopt unmanaged or modified mock files
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MockManifest {
    version: u8,
    openapi_sha256: String,
    files: BTreeMap<String, String>,
    rules_file: String,
    rules_sha256: String,
}

pub fn run(args: MockArgs) -> u8 {
    match run_inner(args) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("serialize mock result")
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

pub(crate) fn automatic(project: &Path, settings: &super::config::MockSettings) -> Result<Value> {
    run_inner(MockArgs {
        project: project.to_owned(),
        output_root: settings.output_root.clone(),
        seed: settings.seed,
        dry_run: false,
        force: false,
    })
}

fn run_inner(args: MockArgs) -> Result<Value> {
    validate_relative_root(&args.output_root)?;
    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("resolve generated project {}", args.project.display()))?;
    let config = super::config::ProjectConfig::load(&project)?;
    let openapi_path = pending_or_stable_openapi(&project)?;
    let openapi_source = fs::read_to_string(&openapi_path)?;
    let openapi = serde_json::from_str::<Value>(&openapi_source)?;
    let app_name = openapi["x-nlab"]["appName"]
        .as_str()
        .context("OpenAPI x-nlab.appName missing")?;
    let mut files = BTreeMap::new();
    let mut rules = Vec::new();
    for (path, path_item) in openapi["paths"]
        .as_object()
        .context("OpenAPI paths missing")?
    {
        for (method, operation) in path_item.as_object().into_iter().flatten() {
            if !is_http_method(method) {
                continue;
            }
            let operation_key = operation["x-nlab-operation-key"]
                .as_str()
                .context("operation key missing")?;
            let facade = operation["x-nlab-facade"].as_str().unwrap_or("Facade");
            let method_name = operation["x-nlab-method-name"]
                .as_str()
                .or_else(|| operation["operationId"].as_str())
                .unwrap_or("operation");
            let Some(schema) = success_schema(operation) else {
                continue;
            };
            let mut rng = operation_rng(args.seed, operation_key);
            let data = generate_value(schema, &openapi, "$", 0, &mut BTreeSet::new(), &mut rng)?;
            let response = response_envelope(&config.frontend.response, data);
            let relative = format!(
                "{}/{}/{}/{}.json",
                args.output_root.trim_end_matches('/'),
                safe_segment(app_name),
                safe_segment(facade),
                safe_segment(method_name)
            );
            files.insert(
                relative.clone(),
                format!("{}\n", serde_json::to_string_pretty(&response)?),
            );
            rules.push(format!(
                "{} file://{}",
                path,
                project.join(&relative).display()
            ));
        }
    }
    rules.sort();
    let rules_file = format!(
        "{}/{}/whistle.rules",
        args.output_root.trim_end_matches('/'),
        safe_segment(app_name)
    );
    let rules_source = format!(
        "# >>> jt nlab-api {app_name}\n{}\n# <<< jt nlab-api {app_name}\n",
        rules.join("\n")
    );
    let manifest_path = project.join(".nlab/mock-manifest.json");
    let previous = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|source| serde_json::from_str::<MockManifest>(&source).ok())
        .filter(|manifest| {
            manifest
                .rules_file
                .starts_with(&format!("{}/", args.output_root.trim_end_matches('/')))
        })
        .unwrap_or_default();
    let hashes = files
        .iter()
        .map(|(path, source)| (path.clone(), sha256(source.as_bytes())))
        .collect::<BTreeMap<_, _>>();
    let manifest = MockManifest {
        version: 1,
        openapi_sha256: sha256(openapi_source.as_bytes()),
        files: hashes,
        rules_file: rules_file.clone(),
        rules_sha256: sha256(rules_source.as_bytes()),
    };
    preflight(&project, &files, &rules_file, &previous, args.force)?;
    preflight_stale(&project, &previous, &manifest, args.force)?;
    if !args.dry_run {
        for (relative, source) in &files {
            atomic_write(&safe_target(&project, relative)?, source)?;
        }
        atomic_write(&safe_target(&project, &rules_file)?, &rules_source)?;
        remove_stale(&project, &previous, &manifest, args.force)?;
        atomic_write(
            &manifest_path,
            &format!("{}\n", serde_json::to_string_pretty(&manifest)?),
        )?;
    }
    Ok(serde_json::json!({
        "status": "complete",
        "operations": files.len(),
        "rules": rules.len(),
        "rulesFile": rules_file,
        "manifest": manifest_path,
        "dryRun": args.dry_run,
        "force": args.force,
        "envelope": {
            "codeField": config.frontend.response.mock_code_field,
            "dataField": config.frontend.response.mock_data_field,
        }
    }))
}

fn response_envelope(config: &super::config::ResponseEnvelope, data: Value) -> Value {
    let code = config
        .success_code
        .parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::String(config.success_code.clone()));
    let mut response = Map::new();
    response.insert(config.mock_code_field.clone(), code);
    response.insert(config.mock_data_field.clone(), data);
    Value::Object(response)
}

fn success_schema(operation: &Value) -> Option<&Value> {
    let responses = operation.get("responses")?.as_object()?;
    for status in ["200", "201", "202", "204"] {
        if let Some(schema) = responses
            .get(status)
            .and_then(|response| response.pointer("/content/application~1json/schema"))
        {
            return Some(schema);
        }
    }
    None
}

fn generate_value(
    schema: &Value,
    document: &Value,
    field_path: &str,
    depth: usize,
    visiting: &mut BTreeSet<String>,
    rng: &mut ChaCha8Rng,
) -> Result<Value> {
    if depth > 32 {
        return Ok(Value::Null);
    }
    for key in ["example", "default", "const"] {
        if let Some(value) = schema.get(key) {
            return Ok(value.clone());
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.is_empty() {
            return Ok(values[rng.random_range(0..values.len())].clone());
        }
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if !visiting.insert(reference.to_owned()) {
            return Ok(Value::Null);
        }
        let target = resolve_reference(document, reference)?;
        let result = generate_value(target, document, field_path, depth + 1, visiting, rng);
        visiting.remove(reference);
        return result;
    }
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        let mut object = Map::new();
        for item in all_of {
            if let Value::Object(values) =
                generate_value(item, document, field_path, depth + 1, visiting, rng)?
            {
                object.extend(values);
            }
        }
        return Ok(Value::Object(object));
    }
    let schema_type = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if schema.get("properties").is_some() {
                "object"
            } else {
                "string"
            }
        });
    match schema_type {
        "object" => {
            let mut object = Map::new();
            for (name, property) in schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                object.insert(
                    name.clone(),
                    generate_value(
                        property,
                        document,
                        &format!("{field_path}.{name}"),
                        depth + 1,
                        visiting,
                        rng,
                    )?,
                );
            }
            Ok(Value::Object(object))
        }
        "array" => {
            let item = schema.get("items").unwrap_or(&Value::Null);
            Ok(Value::Array(
                (0..2)
                    .map(|_| generate_value(item, document, field_path, depth + 1, visiting, rng))
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        "integer" => Ok(json!(rng.random_range(1..=100))),
        "number" => Ok(json!(rng.random_range(1..=1000))),
        "boolean" => Ok(Value::Bool(rng.random_bool(0.5))),
        "null" => Ok(Value::Null),
        _ => Ok(Value::String(semantic_string(field_path, schema, rng))),
    }
}

fn semantic_string(field_path: &str, schema: &Value, rng: &mut ChaCha8Rng) -> String {
    let field = field_path
        .rsplit('.')
        .next()
        .unwrap_or(field_path)
        .to_ascii_lowercase();
    let format = schema.get("format").and_then(Value::as_str).unwrap_or("");
    if format == "date-time" || field.ends_with("time") {
        return "2026-08-21T10:30:00+08:00".to_owned();
    }
    if format == "date" || field.ends_with("date") {
        return "2026-08-21".to_owned();
    }
    if format == "email" || field.contains("email") {
        return "mock@example.com".to_owned();
    }
    if field.contains("phone") || field.contains("mobile") {
        return format!("138{:08}", rng.random_range(0..100_000_000));
    }
    if field.contains("image") || field.contains("pic") || field.contains("avatar") {
        return "https://example.com/mock.png".to_owned();
    }
    if field.ends_with("url") {
        return "https://example.com/mock".to_owned();
    }
    if field.ends_with("id") || field == "uid" {
        return rng.random_range(100_000..=999_999).to_string();
    }
    if field.contains("name") {
        return "示例名称".to_owned();
    }
    format!(
        "MOCK_{}_{:04}",
        safe_segment(&field).to_ascii_uppercase(),
        rng.random_range(0..10_000)
    )
}

fn resolve_reference<'a>(document: &'a Value, reference: &str) -> Result<&'a Value> {
    let pointer = reference
        .strip_prefix('#')
        .context("external OpenAPI references are unsupported")?;
    document
        .pointer(pointer)
        .with_context(|| format!("OpenAPI reference missing: {reference}"))
}

fn preflight(
    project: &Path,
    files: &BTreeMap<String, String>,
    rules_file: &str,
    previous: &MockManifest,
    force: bool,
) -> Result<()> {
    for relative in files.keys() {
        let target = safe_target(project, relative)?;
        if !target.exists() || force {
            continue;
        }
        let expected = previous
            .files
            .get(relative)
            .map(String::as_str)
            .unwrap_or("");
        let current = fs::read(&target)?;
        if expected.is_empty() || sha256(&current) != expected {
            bail!(
                "refuse to overwrite unmanaged or modified mock file: {}",
                target.display()
            );
        }
    }
    let rules_target = safe_target(project, rules_file)?;
    if rules_target.exists() && !force {
        let current = fs::read(&rules_target)?;
        if previous.rules_sha256.is_empty() || sha256(&current) != previous.rules_sha256 {
            bail!(
                "refuse to overwrite unmanaged or modified rules file: {}",
                rules_target.display()
            );
        }
    }
    Ok(())
}

fn remove_stale(
    project: &Path,
    previous: &MockManifest,
    current: &MockManifest,
    force: bool,
) -> Result<()> {
    for (relative, hash) in &previous.files {
        if current.files.contains_key(relative) {
            continue;
        }
        let target = safe_target(project, relative)?;
        if !target.is_file() {
            continue;
        }
        if !force && sha256(&fs::read(&target)?) != *hash {
            bail!("refuse to remove modified mock file: {}", target.display());
        }
        fs::remove_file(target)?;
    }
    Ok(())
}

fn preflight_stale(
    project: &Path,
    previous: &MockManifest,
    current: &MockManifest,
    force: bool,
) -> Result<()> {
    for (relative, hash) in &previous.files {
        if current.files.contains_key(relative) {
            continue;
        }
        let target = safe_target(project, relative)?;
        if !target.is_file() {
            continue;
        }
        if !force && sha256(&fs::read(&target)?) != *hash {
            bail!("refuse to remove modified mock file: {}", target.display());
        }
    }
    Ok(())
}

fn pending_or_stable_openapi(project: &Path) -> Result<PathBuf> {
    for relative in [
        ".nlab/openapi.pending.json",
        ".nlab/openapi.json",
        "openapi.json",
    ] {
        let path = project.join(relative);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("OpenAPI file not found in {}", project.display())
}

fn operation_rng(seed: u64, operation_key: &str) -> ChaCha8Rng {
    let mut digest = Sha256::new();
    digest.update(seed.to_le_bytes());
    digest.update(operation_key.as_bytes());
    ChaCha8Rng::from_seed(digest.finalize().into())
}

fn safe_target(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe mock path: {relative}");
    }
    let mut target = root.to_owned();
    for component in path.components() {
        let Component::Normal(component) = component else {
            unreachable!();
        };
        target.push(component);
        if fs::symlink_metadata(&target)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("mock path crosses symlink: {}", target.display());
        }
    }
    Ok(target)
}

fn validate_relative_root(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("--output-root must be a safe relative directory");
    }
    Ok(())
}

fn atomic_write(path: &Path, source: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, source)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn safe_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn sha256(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_http_method(value: &str) -> bool {
    matches!(value, "get" | "post" | "put" | "patch" | "delete")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_seed_is_repeatable_and_operation_scoped() {
        let mut first = operation_rng(42, "Facade#a");
        let mut second = operation_rng(42, "Facade#a");
        let mut other = operation_rng(42, "Facade#b");
        assert_eq!(first.random::<u64>(), second.random::<u64>());
        assert_ne!(first.random::<u64>(), other.random::<u64>());
    }

    #[test]
    fn semantic_values_are_readable() {
        let mut rng = operation_rng(42, "Facade#query");
        assert_eq!(
            semantic_string("$.userName", &json!({}), &mut rng),
            "示例名称"
        );
        assert_eq!(
            semantic_string("$.createdTime", &json!({}), &mut rng),
            "2026-08-21T10:30:00+08:00"
        );
    }

    #[test]
    fn response_envelope_uses_project_fields() {
        let config = serde_json::from_value::<super::super::config::ResponseEnvelope>(json!({
            "successCode": "0",
            "codeFields": ["code", "respCode"],
            "dataFields": ["data", "respData"],
            "mockCodeField": "respCode",
            "mockDataField": "respData"
        }))
        .unwrap();
        assert_eq!(
            response_envelope(&config, json!({"id": 1})),
            json!({"respCode": 0, "respData": {"id": 1}})
        );
    }
}
