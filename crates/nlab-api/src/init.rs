use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{Map, Value};

use super::config::{
    BackendConfig, BuildToolConfig, BuildToolKind, CONFIG_FILE, CONFIG_VERSION, FrontendConfig,
    ImportAliases, LEGACY_CONFIG_FILE, LOCAL_CONFIG_FILE, LayoutPreset, LocalProjectConfig,
    OutputLayout, ProjectConfig, RequestAdapter, ResponseEnvelope, ResponseMode, STATE_DIR,
    TestConfig,
};

#[derive(Clone, Debug, Args)]
pub struct InitArgs {
    /// Frontend project to inspect and configure
    #[arg(long, value_name = "path", default_value = ".")]
    project: PathBuf,
    /// Existing Java backend repository containing @ServiceContract Facades
    #[arg(
        long,
        value_name = "path",
        required_unless_present = "repo_url",
        conflicts_with = "repo_url"
    )]
    repo_path: Option<PathBuf>,
    /// Git URL cloned when no local backend repository is supplied
    #[arg(
        long,
        value_name = "url",
        required_unless_present = "repo_path",
        conflicts_with = "repo_path"
    )]
    repo_url: Option<String>,
    /// Clone destination; defaults to a repository-specific path under ~/.local/share/nlab-api/repos
    #[arg(long, value_name = "path", requires = "repo_url")]
    clone_dir: Option<PathBuf>,
    /// Backend branch; default: currently checked-out branch
    #[arg(long)]
    branch: Option<String>,
    /// Application name used in placeholder paths; default: backend directory name
    #[arg(long)]
    app_name: Option<String>,
    /// Generated implementation directory family; default: detect existing api/service layout
    #[arg(long, value_enum)]
    layout: Option<LayoutPreset>,
    /// Overall deadline, capped at 1200 seconds
    #[arg(long, default_value_t = super::MAX_TIMEOUT_SECONDS)]
    timeout_seconds: u64,
}

pub fn run(args: InitArgs) -> u8 {
    match run_inner(args) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("serialize init result")
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

fn run_inner(args: InitArgs) -> Result<Value> {
    if args.timeout_seconds == 0 || args.timeout_seconds > super::MAX_TIMEOUT_SECONDS {
        bail!(
            "--timeout-seconds must be between 1 and {}",
            super::MAX_TIMEOUT_SECONDS
        );
    }
    let deadline = Instant::now() + Duration::from_secs(args.timeout_seconds);
    let project = args
        .project
        .canonicalize()
        .with_context(|| format!("resolve frontend project {}", args.project.display()))?;
    if !project.join("package.json").is_file() {
        bail!("frontend package.json missing: {}", project.display());
    }
    let repository_path = match (&args.repo_path, &args.repo_url) {
        (Some(path), None) => super::repo::resolve_path(path, None)?,
        (None, Some(repository)) => match &args.clone_dir {
            Some(path) => super::repo::resolve_path(path, Some(repository))?,
            None => super::repo::managed_clone_path(repository)?,
        },
        _ => unreachable!("clap requires exactly one backend repository input"),
    };
    let prepared = super::repo::prepare(
        &repository_path,
        args.repo_url.as_deref(),
        args.branch.as_deref(),
        deadline,
    )?;
    let backend = prepared.target.clone();
    let contract_roots = detect_contract_roots(&backend.root)?;
    let previous = (project.join(CONFIG_FILE).is_file()
        || project.join(LEGACY_CONFIG_FILE).is_file())
    .then(|| ProjectConfig::load(&project))
    .transpose()?;
    let frontend = probe_frontend(&project, args.layout, previous.as_ref())?;
    let app_name = match (args.app_name, args.repo_url.as_deref()) {
        (Some(app_name), _) => app_name,
        (None, Some(repository)) => super::repo::repository_name(repository)?.to_owned(),
        (None, None) => backend
            .root
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "nlab".to_owned()),
    };
    let config = ProjectConfig {
        version: CONFIG_VERSION,
        backend: BackendConfig {
            repository: Some(prepared.origin.clone()),
            repo_path: backend.root.clone(),
            branch: backend.branch,
            app_name,
            contract_roots,
        },
        frontend,
        gateway: previous
            .as_ref()
            .map(|config| config.gateway.clone())
            .unwrap_or_default(),
        migration: previous
            .as_ref()
            .map(|config| config.migration.clone())
            .unwrap_or_default(),
        mock: previous
            .as_ref()
            .map(|config| config.mock.clone())
            .unwrap_or_default(),
        after_generate: previous
            .as_ref()
            .map(|config| config.after_generate.clone())
            .unwrap_or_default(),
    };
    config.validate()?;
    ensure_state_directory(&project)?;

    let build_config = project.join(&config.frontend.build_tool.config_path);
    let tsconfig = project.join(&config.frontend.tsconfig_path);
    let vite_source = fs::read_to_string(&build_config)
        .with_context(|| format!("read Vite config {}", build_config.display()))?;
    let tsconfig_source = fs::read_to_string(&tsconfig)
        .with_context(|| format!("read TypeScript config {}", tsconfig.display()))?;
    let vite_patched = patch_vite_aliases(&vite_source, &config.frontend)?;
    let tsconfig_patched = patch_tsconfig_aliases(&tsconfig_source, &config.frontend)?;
    let config_source = config.shared_source()?;
    let mut local_config = LocalProjectConfig::load(&project)?;
    local_config.backend.repo_path = Some(backend.root.clone());
    let local_config_source = local_config.source()?;
    let mut changes = vec![
        FileChange::existing(build_config.clone(), vite_source, vite_patched),
        FileChange::existing(tsconfig.clone(), tsconfig_source, tsconfig_patched),
    ];
    let mut updated = vec![build_config.clone(), tsconfig.clone()];
    for test_config in config
        .frontend
        .build_tool
        .test_configs
        .iter()
        .filter(|test_config| !test_config.inherits_build_config)
    {
        let path = project.join(&test_config.path);
        let source = fs::read_to_string(&path)
            .with_context(|| format!("read test config {}", path.display()))?;
        let patched = patch_vite_aliases(&source, &config.frontend).with_context(|| {
            format!(
                "test config {} neither inherits Vite config nor exposes resolve.alias",
                path.display()
            )
        })?;
        changes.push(FileChange::existing(path.clone(), source, patched));
        updated.push(path);
    }
    let config_path = project.join(CONFIG_FILE);
    changes.push(FileChange::load(config_path.clone(), config_source)?);
    updated.push(config_path.clone());
    let local_config_path = project.join(LOCAL_CONFIG_FILE);
    changes.push(FileChange::load(
        local_config_path.clone(),
        local_config_source,
    )?);
    updated.push(local_config_path.clone());
    super::config::ensure_local_config_ignored(&project)?;
    apply_changes(changes)?;
    remove_legacy_config(&project)?;

    Ok(serde_json::json!({
        "status": "complete",
        "project": project,
        "config": config_path,
        "localConfig": local_config_path,
        "backend": config.backend.repo_path,
        "branch": config.backend.branch,
        "contractRoots": config.backend.contract_roots,
        "buildTool": config.frontend.build_tool.kind,
        "request": {
            "module": config.frontend.request.module,
            "export": config.frontend.request.export,
            "responseMode": config.frontend.request.response_mode,
        },
        "layout": config.frontend.layout,
        "aliases": config.frontend.aliases,
        "gateway": config.gateway,
        "migration": config.migration,
        "mock": config.mock,
        "updated": updated,
    }))
}

fn ensure_state_directory(project: &Path) -> Result<()> {
    let path = project.join(STATE_DIR);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!(
                "nlab-api state path must be a real directory: {}",
                path.display()
            )
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&path)
            .with_context(|| format!("create nlab-api state directory {}", path.display())),
        Err(error) => Err(error.into()),
    }
}

fn remove_legacy_config(project: &Path) -> Result<()> {
    let path = project.join(LEGACY_CONFIG_FILE);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("refuse to remove non-file or symlink: {}", path.display());
    }
    let source = fs::read_to_string(&path)?;
    serde_json::from_str::<ProjectConfig>(&source)
        .with_context(|| format!("legacy nlab-api config is not managed: {}", path.display()))?;
    fs::remove_file(&path).with_context(|| format!("remove legacy config {}", path.display()))
}

fn probe_frontend(
    project: &Path,
    requested_layout: Option<LayoutPreset>,
    previous: Option<&ProjectConfig>,
) -> Result<FrontendConfig> {
    let tsconfig_path = ["tsconfig.json", "tsconfig.app.json"]
        .into_iter()
        .find(|candidate| project.join(candidate).is_file())
        .context("TypeScript config not found")?
        .to_owned();
    let source_root = detect_source_root(project, &tsconfig_path)?;
    let vite_config = [
        "vite.config.ts",
        "vite.config.mts",
        "vite.config.js",
        "vite.config.mjs",
    ]
    .into_iter()
    .find(|candidate| project.join(candidate).is_file())
    .context("Vite config not found")?
    .to_owned();
    let test_configs = ["vitest.config.ts", "vitest.config.mts", "vitest.config.js"]
        .into_iter()
        .filter(|candidate| project.join(candidate).is_file())
        .map(|path| {
            let source = fs::read_to_string(project.join(path))
                .with_context(|| format!("read test config {path}"))?;
            Ok(TestConfig {
                path: path.to_owned(),
                inherits_build_config: source.contains("mergeConfig")
                    && source.contains(&vite_config),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let layout = detect_layout(project, &source_root, requested_layout, previous);
    let aliases = previous
        .filter(|_| requested_layout.is_none())
        .map(|config| config.frontend.aliases.clone())
        .unwrap_or_else(|| aliases_for(layout.preset));
    let (request, response) = detect_request(project, &source_root)?;
    Ok(FrontendConfig {
        source_root,
        build_tool: BuildToolConfig {
            kind: BuildToolKind::Vite,
            config_path: vite_config,
            test_configs,
        },
        tsconfig_path,
        request,
        response,
        layout,
        aliases,
    })
}

fn detect_source_root(project: &Path, tsconfig_path: &str) -> Result<String> {
    let source = fs::read_to_string(project.join(tsconfig_path))
        .with_context(|| format!("read TypeScript config {tsconfig_path}"))?;
    let value = serde_json::from_str::<Value>(&source).context("decode TypeScript config JSON")?;
    let configured = value["compilerOptions"]["paths"]["@/*"]
        .as_array()
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(|value| {
            value
                .trim_start_matches("./")
                .trim_end_matches("/*")
                .to_owned()
        });
    let source_root = configured.unwrap_or_else(|| "src".to_owned());
    if !project.join(&source_root).is_dir() {
        bail!(
            "frontend source root missing: {}",
            project.join(&source_root).display()
        );
    }
    Ok(source_root)
}

fn detect_layout(
    project: &Path,
    source_root: &str,
    requested: Option<LayoutPreset>,
    previous: Option<&ProjectConfig>,
) -> OutputLayout {
    if requested.is_none()
        && let Some(previous) = previous
    {
        return previous.frontend.layout.clone();
    }
    let preset = requested.unwrap_or_else(|| {
        if project.join(source_root).join("api").is_dir()
            && !project.join(source_root).join("service").is_dir()
        {
            LayoutPreset::Api
        } else {
            LayoutPreset::Service
        }
    });
    let noun = preset.noun();
    let legacy_types = format!("{source_root}/types/{noun}-type");
    let plural_types = format!("{source_root}/types/{noun}-types");
    let types_dir = if project.join(&legacy_types).is_dir() {
        legacy_types
    } else {
        plural_types
    };
    OutputLayout {
        preset,
        implementation_dir: format!("{source_root}/{noun}"),
        types_dir,
        enums_dir: format!("{source_root}/types/{noun}-enums"),
    }
}

fn aliases_for(preset: LayoutPreset) -> ImportAliases {
    let noun = preset.noun();
    ImportAliases {
        implementation: format!("@{noun}"),
        types: format!("@{noun}-types"),
        enums: format!("@{noun}-enums"),
    }
}

fn detect_request(project: &Path, source_root: &str) -> Result<(RequestAdapter, ResponseEnvelope)> {
    let root = project.join(source_root);
    let mut candidates = WalkBuilder::new(&root)
        .standard_filters(true)
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter(|entry| {
            matches!(
                entry.path().extension().and_then(|value| value.to_str()),
                Some("ts" | "tsx")
            )
        })
        .filter_map(|entry| {
            let source = fs::read_to_string(entry.path()).ok()?;
            (source.contains("export function nlabRequest")
                || source.contains("export const nlabRequest"))
            .then(|| (entry.into_path(), source))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    let (path, source) = match candidates.as_slice() {
        [candidate] => candidate,
        [] => bail!("no exported nlabRequest adapter found under {source_root}"),
        _ => bail!(
            "multiple nlabRequest adapters found: {}",
            candidates
                .iter()
                .map(|(path, _)| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let relative = path
        .strip_prefix(&root)?
        .with_extension("")
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    let module = format!("@/{relative}")
        .trim_end_matches("/index")
        .to_owned();
    let code_fields = ["code", "respCode"]
        .into_iter()
        .filter(|field| has_interface_field(source, field))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let data_fields = ["data", "respData"]
        .into_iter()
        .filter(|field| has_interface_field(source, field))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if code_fields.is_empty() || data_fields.is_empty() {
        bail!("nlabRequest response envelope fields could not be detected");
    }
    let success_code =
        Regex::new(r#"businessCode\.toString\(\)\s*!==\s*['\"](?P<code>[^'\"]+)['\"]"#)?
            .captures(source)
            .and_then(|captures| captures.name("code"))
            .map(|value| value.as_str().to_owned())
            .unwrap_or_else(|| "0".to_owned());
    let (mock_code_field, mock_data_field) =
        detect_mock_envelope(project, source_root, &code_fields, &data_fields);
    Ok((
        RequestAdapter {
            module,
            export: "nlabRequest".to_owned(),
            response_mode: ResponseMode::Unwrapped,
        },
        ResponseEnvelope {
            success_code,
            code_fields,
            data_fields,
            mock_code_field,
            mock_data_field,
        },
    ))
}

fn detect_mock_envelope(
    project: &Path,
    source_root: &str,
    code_fields: &[String],
    data_fields: &[String],
) -> (String, String) {
    let mut builder = WalkBuilder::new(project.join(source_root).join("mock"));
    builder.add(project.join("mock"));
    let mut counts = BTreeMap::<(String, String), usize>::new();
    for entry in builder
        .standard_filters(true)
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
    {
        let Some(object) = fs::read_to_string(entry.path())
            .ok()
            .and_then(|source| serde_json::from_str::<Value>(&source).ok())
            .and_then(|value| value.as_object().cloned())
        else {
            continue;
        };
        for code in code_fields
            .iter()
            .filter(|field| object.contains_key(*field))
        {
            for data in data_fields
                .iter()
                .filter(|field| object.contains_key(*field))
            {
                *counts.entry((code.clone(), data.clone())).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)))
        .map(|(fields, _)| fields)
        .unwrap_or_else(|| (code_fields[0].clone(), data_fields[0].clone()))
}

fn has_interface_field(source: &str, field: &str) -> bool {
    Regex::new(&format!(r"(?m)^\s*{}\??\s*:", regex::escape(field)))
        .expect("valid field regex")
        .is_match(source)
}

fn detect_contract_roots(repo: &Path) -> Result<Vec<String>> {
    let parents = WalkBuilder::new(repo)
        .standard_filters(true)
        .hidden(false)
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.ends_with("Facade.java"))
        })
        .filter_map(|entry| {
            let source = fs::read_to_string(entry.path()).ok()?;
            source
                .contains("ServiceContract")
                .then(|| entry.path().parent().map(Path::to_owned))
                .flatten()
        })
        .collect::<BTreeSet<_>>();
    if parents.is_empty() {
        bail!("no @ServiceContract Facade declarations found");
    }
    let mut roots = BTreeSet::new();
    for parent in parents {
        let mut root = parent;
        while root.file_name().and_then(|value| value.to_str()) != Some("contract") {
            if !root.pop() || root == repo {
                bail!("Facade declaration is not below one semantic contract package");
            }
        }
        let relative = root.strip_prefix(repo)?;
        let value = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if !value.contains("src/main/java/") {
            bail!("Facade contract root is too broad: {value}");
        }
        roots.insert(value);
    }
    if roots.is_empty() {
        bail!("no semantic contract roots found");
    }
    for root in &roots {
        if roots
            .iter()
            .any(|candidate| candidate != root && Path::new(root).starts_with(candidate))
        {
            bail!("Facade declarations are not below one semantic contract package");
        }
    }
    Ok(roots.into_iter().collect())
}

fn patch_vite_aliases(source: &str, frontend: &FrontendConfig) -> Result<String> {
    let alias_start =
        Regex::new(r#"(?m)^(?P<indent>[ \t]*)(?:alias|['\"]alias['\"])[ \t]*:[ \t]*\{"#)?
            .captures(source)
            .context("Vite resolve.alias object not found")?;
    let whole = alias_start.get(0).expect("whole alias match");
    let open = whole.start()
        + source[whole.start()..whole.end()]
            .rfind('{')
            .expect("alias opening brace");
    let close = matching_brace(source, open).context("Vite resolve.alias object is incomplete")?;
    let block = &source[open + 1..close];
    let base =
        Regex::new(r#"(?m)^(?P<indent>[ \t]*)['\"]@['\"][ \t]*:[ \t]*(?P<value>.+),[ \t]*$"#)?
            .captures(block)
            .context("Vite @ source alias not found")?;
    let indent = base.name("indent").expect("alias indent").as_str();
    let base_value = base.name("value").expect("alias value").as_str().trim();
    let source_marker = format!("./{}", frontend.source_root.trim_matches('/'));
    if !base_value.contains(&source_marker) {
        bail!("Vite @ alias does not contain {source_marker}");
    }
    let aliases = [
        (
            &frontend.aliases.implementation,
            &frontend.layout.implementation_dir,
        ),
        (&frontend.aliases.types, &frontend.layout.types_dir),
        (&frontend.aliases.enums, &frontend.layout.enums_dir),
    ];
    let mut additions = String::new();
    for (alias, target) in aliases {
        let existing = Regex::new(&format!(
            r#"(?m)^[ \t]*['\"]{}['\"][ \t]*:[ \t]*(?P<value>.+),?[ \t]*$"#,
            regex::escape(alias)
        ))?
        .captures(block);
        let target_marker = format!("./{}", target.trim_matches('/'));
        if let Some(existing) = existing {
            if !existing
                .name("value")
                .is_some_and(|value| value.as_str().contains(&target_marker))
            {
                bail!("Vite alias {alias} points outside {target_marker}");
            }
            continue;
        }
        let value = base_value.replacen(&source_marker, &target_marker, 1);
        additions.push_str(&format!("{indent}'{alias}': {value},\n"));
    }
    if additions.is_empty() {
        return Ok(source.to_owned());
    }
    let insertion = open + 1;
    let rest = if source.as_bytes().get(insertion) == Some(&b'\n') {
        insertion + 1
    } else {
        insertion
    };
    let mut output = String::with_capacity(source.len() + additions.len() + 1);
    output.push_str(&source[..insertion]);
    output.push('\n');
    output.push_str(&additions);
    output.push_str(&source[rest..]);
    Ok(output)
}

fn patch_tsconfig_aliases(source: &str, frontend: &FrontendConfig) -> Result<String> {
    let mut value =
        serde_json::from_str::<Value>(source).context("decode TypeScript config JSON")?;
    let compiler_options = value
        .as_object_mut()
        .context("TypeScript config root must be an object")?
        .entry("compilerOptions")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("compilerOptions must be an object")?;
    let paths = compiler_options
        .entry("paths")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("compilerOptions.paths must be an object")?;
    for (alias, target) in [
        (
            &frontend.aliases.implementation,
            &frontend.layout.implementation_dir,
        ),
        (&frontend.aliases.types, &frontend.layout.types_dir),
        (&frontend.aliases.enums, &frontend.layout.enums_dir),
    ] {
        let key = format!("{alias}/*");
        let expected = serde_json::json!([format!("./{}/*", target.trim_matches('/'))]);
        if let Some(existing) = paths.get(&key) {
            if existing != &expected {
                bail!("TypeScript alias {key} has conflicting target");
            }
        } else {
            paths.insert(key, expected);
        }
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut index = open;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
        } else if block_comment {
            if byte == b'*' && next == Some(b'/') {
                block_comment = false;
                index += 1;
            }
        } else if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
        } else if byte == b'/' && next == Some(b'/') {
            line_comment = true;
            index += 1;
        } else if byte == b'/' && next == Some(b'*') {
            block_comment = true;
            index += 1;
        } else if matches!(byte, b'\'' | b'\"' | b'`') {
            quote = Some(byte);
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

struct FileChange {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

impl FileChange {
    fn existing(path: PathBuf, before: String, after: String) -> Self {
        Self {
            path,
            before: Some(before.into_bytes()),
            after: after.into_bytes(),
        }
    }

    fn load(path: PathBuf, after: String) -> Result<Self> {
        let before = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("refuse to replace symlink: {}", path.display())
            }
            Ok(metadata) if metadata.is_file() => Some(fs::read(&path)?),
            Ok(_) => bail!("refuse to replace non-file: {}", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            before,
            after: after.into_bytes(),
        })
    }
}

fn apply_changes(changes: Vec<FileChange>) -> Result<()> {
    let changes = changes
        .into_iter()
        .filter(|change| change.before.as_deref() != Some(change.after.as_slice()))
        .collect::<Vec<_>>();
    for change in &changes {
        if fs::symlink_metadata(&change.path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("refuse to replace symlink: {}", change.path.display());
        }
    }
    let mut completed: Vec<usize> = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        if let Err(error) = atomic_write(&change.path, &change.after) {
            for completed_index in completed.into_iter().rev() {
                let completed_change = &changes[completed_index];
                match &completed_change.before {
                    Some(before) => {
                        let _ = atomic_write(&completed_change.path, before);
                    }
                    None => {
                        let _ = fs::remove_file(&completed_change.path);
                    }
                }
            }
            return Err(error);
        }
        completed.push(index);
    }
    Ok(())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("output path has no parent: {}", path.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("create temporary file for {}", path.display()))?;
    temporary.write_all(content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o7777)
            .unwrap_or(0o644);
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontend() -> FrontendConfig {
        FrontendConfig {
            source_root: "src".to_owned(),
            build_tool: BuildToolConfig {
                kind: BuildToolKind::Vite,
                config_path: "vite.config.ts".to_owned(),
                test_configs: vec![],
            },
            tsconfig_path: "tsconfig.json".to_owned(),
            request: RequestAdapter {
                module: "@/utils/request".to_owned(),
                export: "nlabRequest".to_owned(),
                response_mode: ResponseMode::Unwrapped,
            },
            response: ResponseEnvelope {
                success_code: "0".to_owned(),
                code_fields: vec!["code".to_owned(), "respCode".to_owned()],
                data_fields: vec!["data".to_owned(), "respData".to_owned()],
                mock_code_field: "respCode".to_owned(),
                mock_data_field: "respData".to_owned(),
            },
            layout: OutputLayout {
                preset: LayoutPreset::Service,
                implementation_dir: "src/service".to_owned(),
                types_dir: "src/types/service-type".to_owned(),
                enums_dir: "src/types/service-enums".to_owned(),
            },
            aliases: aliases_for(LayoutPreset::Service),
        }
    }

    #[test]
    fn vite_alias_patch_is_idempotent() {
        let source = "resolve: {\n  alias: {\n    '@': fileURLToPath(new URL('./src', import.meta.url)),\n  },\n},\n";
        let once = patch_vite_aliases(source, &frontend()).unwrap();
        let twice = patch_vite_aliases(&once, &frontend()).unwrap();
        assert_eq!(once, twice);
        assert!(once.contains("'@service': fileURLToPath(new URL('./src/service'"));
        assert!(
            once.contains("'@service-enums': fileURLToPath(new URL('./src/types/service-enums'")
        );
        assert!(!once.contains("\n\n"), "{once:?}");
    }

    #[test]
    fn tsconfig_alias_patch_is_idempotent() {
        let source = r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#;
        let once = patch_tsconfig_aliases(source, &frontend()).unwrap();
        let twice = patch_tsconfig_aliases(&once, &frontend()).unwrap();
        assert_eq!(once, twice);
        let value = serde_json::from_str::<Value>(&once).unwrap();
        assert_eq!(
            value["compilerOptions"]["paths"]["@service-types/*"],
            serde_json::json!(["./src/types/service-type/*"])
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_existing_mode_and_uses_readable_default() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("existing.ts");
        fs::write(&existing, "before").unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&existing, b"after").unwrap();
        assert_eq!(
            fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
            0o640
        );

        let created = root.path().join("created.json");
        atomic_write(&created, b"{}").unwrap();
        assert_eq!(
            fs::metadata(&created).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn contract_probe_stops_at_semantic_contract_package() {
        let root = tempfile::tempdir().unwrap();
        for relative in [
            "contract/src/main/java/p/contract/checkapp/IGoodsFacade.java",
            "contract/src/main/java/p/contract/operations/IOrderFacade.java",
            "other/src/main/java/q/contract/IOtherFacade.java",
        ] {
            let path = root.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "@ServiceContract public interface Facade {}").unwrap();
        }
        assert_eq!(
            detect_contract_roots(root.path()).unwrap(),
            vec![
                "contract/src/main/java/p/contract",
                "other/src/main/java/q/contract"
            ]
        );
    }

    #[test]
    fn mock_envelope_probe_reuses_existing_project_convention() {
        let root = tempfile::tempdir().unwrap();
        let mock = root.path().join("src/mock/example.json");
        fs::create_dir_all(mock.parent().unwrap()).unwrap();
        fs::write(&mock, r#"{"respCode":0,"respData":{}}"#).unwrap();
        let fields = detect_mock_envelope(
            root.path(),
            "src",
            &["code".to_owned(), "respCode".to_owned()],
            &["data".to_owned(), "respData".to_owned()],
        );
        assert_eq!(fields, ("respCode".to_owned(), "respData".to_owned()));
    }
}
