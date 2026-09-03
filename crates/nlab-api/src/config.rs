use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Deserializer, Serialize};

pub const STATE_DIR: &str = ".nlab";
pub const CONFIG_FILE: &str = ".nlab/nlab-api.config.json";
pub const LOCAL_CONFIG_FILE: &str = ".nlab/nlab-api.local.json";
pub const LOCAL_GITIGNORE_ENTRY: &str = "/.nlab/nlab-api.local.json";
pub const LEGACY_CONFIG_FILE: &str = "nlab-api.config.json";
pub const LEGACY_LOCAL_CONFIG_FILE: &str = ".nlab/cli.local.json";
pub const CONFIG_VERSION: u8 = 2;
const LOCAL_CONFIG_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub version: u8,
    pub backend: BackendConfig,
    pub frontend: FrontendConfig,
    #[serde(default)]
    pub gateway: GatewaySettings,
    #[serde(default)]
    pub migration: MigrationSettings,
    #[serde(default)]
    pub mock: MockSettings,
    #[serde(default)]
    pub after_generate: Vec<AfterGenerateHook>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterGenerateHook {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub include_generated_files: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    pub enabled: bool,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationSettings {
    pub enabled: bool,
}

impl Default for MigrationSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MockSettings {
    pub enabled: bool,
    pub output_root: String,
    pub seed: u64,
}

impl Default for MockSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            output_root: ".nlab/generated-mock".to_owned(),
            seed: 42,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    pub repo_path: PathBuf,
    pub branch: String,
    pub app_name: String,
    #[serde(
        alias = "contractRoot",
        deserialize_with = "deserialize_contract_roots"
    )]
    pub contract_roots: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum LocalRunner {
    Jt,
    NlabApi,
}

impl LocalRunner {
    pub fn command(self) -> &'static str {
        match self {
            Self::Jt => "jt",
            Self::NlabApi => "nlab-api",
        }
    }
}

#[derive(Clone, Debug, Args)]
pub struct ConfigArgs {
    /// Frontend project owning the local runner preference
    #[arg(long, value_name = "path", default_value = ".")]
    project: PathBuf,
    /// Persist the command used for nlab-api operations
    #[arg(
        long,
        value_enum,
        required_unless_present_any = ["detect", "unset", "show"],
        conflicts_with_all = ["detect", "unset", "show"]
    )]
    runner: Option<LocalRunner>,
    /// Clear the preference; the Skill will detect a runner on its next use
    #[arg(
        long,
        required_unless_present_any = ["detect", "runner", "show"],
        conflicts_with_all = ["detect", "runner", "show"]
    )]
    unset: bool,
    /// Print the resolved local config without changing it
    #[arg(
        long,
        required_unless_present_any = ["detect", "runner", "unset"],
        conflicts_with_all = ["detect", "runner", "unset"]
    )]
    show: bool,
    /// Detect jt first, then nlab-api, only when no runner is configured
    #[arg(
        long,
        required_unless_present_any = ["runner", "show", "unset"],
        conflicts_with_all = ["runner", "show", "unset"]
    )]
    detect: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalBackendConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_path: Option<PathBuf>,
}

impl LocalBackendConfig {
    fn is_empty(&self) -> bool {
        self.repo_path.is_none()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalProjectConfig {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<LocalRunner>,
    #[serde(default, skip_serializing_if = "LocalBackendConfig::is_empty")]
    pub backend: LocalBackendConfig,
}

impl Default for LocalProjectConfig {
    fn default() -> Self {
        Self {
            version: LOCAL_CONFIG_VERSION,
            runner: None,
            backend: LocalBackendConfig::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyLocalConfig {
    version: u8,
    runner: LocalRunner,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ContractRoots {
    One(String),
    Many(Vec<String>),
}

fn deserialize_contract_roots<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match ContractRoots::deserialize(deserializer)? {
        ContractRoots::One(root) => vec![root],
        ContractRoots::Many(roots) => roots,
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendConfig {
    pub source_root: String,
    pub build_tool: BuildToolConfig,
    pub tsconfig_path: String,
    pub request: RequestAdapter,
    pub response: ResponseEnvelope,
    pub layout: OutputLayout,
    pub aliases: ImportAliases,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildToolConfig {
    pub kind: BuildToolKind,
    pub config_path: String,
    pub test_configs: Vec<TestConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildToolKind {
    Vite,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestConfig {
    pub path: String,
    pub inherits_build_config: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestAdapter {
    pub module: String,
    pub export: String,
    pub response_mode: ResponseMode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseMode {
    Unwrapped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseEnvelope {
    pub success_code: String,
    pub code_fields: Vec<String>,
    pub data_fields: Vec<String>,
    #[serde(default = "default_mock_code_field")]
    pub mock_code_field: String,
    #[serde(default = "default_mock_data_field")]
    pub mock_data_field: String,
}

fn default_mock_code_field() -> String {
    "respCode".to_owned()
}

fn default_mock_data_field() -> String {
    "respData".to_owned()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputLayout {
    pub preset: LayoutPreset,
    pub implementation_dir: String,
    pub types_dir: String,
    pub enums_dir: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LayoutPreset {
    Api,
    Service,
}

impl LayoutPreset {
    pub fn noun(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Service => "service",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportAliases {
    pub implementation: String,
    pub types: String,
    pub enums: String,
}

impl LocalProjectConfig {
    pub fn load(project: &Path) -> Result<Self> {
        let path = project.join(LOCAL_CONFIG_FILE);
        reject_symlink_path(project, &path)?;
        if regular_file_exists(&path, "local nlab-api config")? {
            let config: Self = read_json(&path, "local nlab-api config")?;
            config.validate()?;
            return Ok(config);
        }

        let legacy_path = project.join(LEGACY_LOCAL_CONFIG_FILE);
        reject_symlink_path(project, &legacy_path)?;
        if !regular_file_exists(&legacy_path, "legacy local nlab-api config")? {
            return Ok(Self::default());
        }
        let legacy: LegacyLocalConfig = read_json(&legacy_path, "legacy local nlab-api config")?;
        if legacy.version != LOCAL_CONFIG_VERSION {
            bail!(
                "unsupported legacy local nlab-api config version {}; expected {LOCAL_CONFIG_VERSION}",
                legacy.version
            );
        }
        Ok(Self {
            runner: Some(legacy.runner),
            ..Self::default()
        })
    }

    pub fn source(&self) -> Result<String> {
        self.validate()?;
        Ok(format!("{}\n", serde_json::to_string_pretty(self)?))
    }

    pub fn save(&self, project: &Path) -> Result<()> {
        let path = project.join(LOCAL_CONFIG_FILE);
        reject_symlink_path(project, &path)?;
        if self.is_empty() {
            return remove_file_if_present(&path);
        }
        fs::create_dir_all(project.join(STATE_DIR))
            .with_context(|| format!("create nlab-api state directory in {}", project.display()))?;
        atomic_write(&path, self.source()?.as_bytes())
    }

    pub fn is_empty(&self) -> bool {
        self.runner.is_none() && self.backend.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != LOCAL_CONFIG_VERSION {
            bail!(
                "unsupported local nlab-api config version {}; expected {LOCAL_CONFIG_VERSION}",
                self.version
            );
        }
        if self
            .backend
            .repo_path
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            bail!("local backend.repoPath must be absolute");
        }
        Ok(())
    }
}

pub fn ensure_local_config_ignored(project: &Path) -> Result<()> {
    let path = project.join(".gitignore");
    reject_symlink_path(project, &path)?;
    let mut source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    if source
        .lines()
        .any(|line| line.trim() == LOCAL_GITIGNORE_ENTRY)
    {
        return Ok(());
    }
    if !source.is_empty() && !source.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(LOCAL_GITIGNORE_ENTRY);
    source.push('\n');
    atomic_write(&path, source.as_bytes())
}

pub fn remove_legacy_local_config(project: &Path) -> Result<()> {
    let path = project.join(LEGACY_LOCAL_CONFIG_FILE);
    reject_symlink_path(project, &path)?;
    remove_file_if_present(&path)
}

pub fn configure(args: ConfigArgs) -> u8 {
    match configure_inner(args) {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

fn configure_inner(args: ConfigArgs) -> Result<String> {
    configure_inner_with(args, runner_available)
}

fn configure_inner_with(
    args: ConfigArgs,
    available: impl Fn(LocalRunner) -> bool,
) -> Result<String> {
    let project = resolve_project(&args.project)?;
    let mut config = LocalProjectConfig::load(&project)?;
    if args.show {
        return serde_json::to_string_pretty(&config).context("encode local nlab-api config");
    }

    if args.unset {
        ensure_local_config_ignored(&project)?;
        config.runner = None;
        config.save(&project)?;
        remove_legacy_local_config(&project)?;
        return Ok(format!(
            "nlab-api runner preference cleared for {}",
            project.display()
        ));
    }

    let runner = if args.detect {
        if let Some(runner) = config.runner {
            return Ok(format!(
                "nlab-api runner already configured as {} for {}",
                runner.command(),
                project.display()
            ));
        }
        detect_runner(available)?
    } else {
        args.runner.expect("clap requires one config action")
    };
    ensure_local_config_ignored(&project)?;
    config.runner = Some(runner);
    config.save(&project)?;
    remove_legacy_local_config(&project)?;
    Ok(format!(
        "nlab-api runner configured as {} for {}",
        runner.command(),
        project.display()
    ))
}

fn detect_runner(available: impl Fn(LocalRunner) -> bool) -> Result<LocalRunner> {
    if available(LocalRunner::Jt) {
        Ok(LocalRunner::Jt)
    } else if available(LocalRunner::NlabApi) {
        Ok(LocalRunner::NlabApi)
    } else {
        bail!("neither jt nor nlab-api is available")
    }
}

fn runner_available(runner: LocalRunner) -> bool {
    let mut command = Command::new(runner.command());
    if runner == LocalRunner::Jt {
        command.arg("nlab-api");
    }
    command
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn resolve_project(project: &Path) -> Result<PathBuf> {
    let project = if project.is_absolute() {
        project.to_path_buf()
    } else {
        std::env::current_dir()?.join(project)
    };
    let project = project
        .canonicalize()
        .with_context(|| format!("resolve frontend project {}", project.display()))?;
    if !project.is_dir() {
        bail!("frontend project is not a directory: {}", project.display());
    }
    Ok(project)
}

impl ProjectConfig {
    pub fn load(project: &Path) -> Result<Self> {
        let configured = project.join(CONFIG_FILE);
        let legacy = project.join(LEGACY_CONFIG_FILE);
        let path = if configured.is_file() {
            configured
        } else {
            legacy
        };
        let source = fs::read_to_string(&path)
            .with_context(|| format!("read nlab-api config {}", path.display()))?;
        let mut config = serde_json::from_str::<Self>(&source)
            .with_context(|| format!("decode nlab-api config {}", path.display()))?;
        if config.version == CONFIG_VERSION && !config.backend.repo_path.as_os_str().is_empty() {
            bail!(
                "shared nlab-api config must not contain backend.repoPath; move it to {LOCAL_CONFIG_FILE}"
            );
        }
        if let Some(repo_path) = LocalProjectConfig::load(project)?.backend.repo_path {
            config.backend.repo_path = repo_path;
        }
        config.validate()?;
        Ok(config)
    }

    pub fn shared_source(&self) -> Result<String> {
        let mut shared = self.clone();
        shared.version = CONFIG_VERSION;
        shared.backend.repo_path = PathBuf::new();
        shared.validate()?;
        Ok(format!("{}\n", serde_json::to_string_pretty(&shared)?))
    }

    pub fn validate(&self) -> Result<()> {
        match self.version {
            1 if self.backend.repo_path.as_os_str().is_empty() => {
                bail!("nlab-api config version 1 requires backend.repoPath")
            }
            2 if self
                .backend
                .repository
                .as_deref()
                .is_none_or(|repository| repository.trim().is_empty()) =>
            {
                bail!("nlab-api config version 2 requires backend.repository")
            }
            1 | 2 => {}
            version => bail!("unsupported nlab-api config version {version}; expected 1 or 2"),
        }
        if !self.backend.repo_path.as_os_str().is_empty() && !self.backend.repo_path.is_absolute() {
            bail!("backend.repoPath must be absolute");
        }
        if self.backend.contract_roots.is_empty() {
            bail!("backend.contractRoots must not be empty");
        }
        for root in &self.backend.contract_roots {
            validate_relative_path(root, "backend.contractRoots")?;
        }
        for (name, value) in [
            ("frontend.sourceRoot", self.frontend.source_root.as_str()),
            (
                "frontend.buildTool.configPath",
                self.frontend.build_tool.config_path.as_str(),
            ),
            (
                "frontend.tsconfigPath",
                self.frontend.tsconfig_path.as_str(),
            ),
            (
                "frontend.layout.implementationDir",
                self.frontend.layout.implementation_dir.as_str(),
            ),
            (
                "frontend.layout.typesDir",
                self.frontend.layout.types_dir.as_str(),
            ),
            (
                "frontend.layout.enumsDir",
                self.frontend.layout.enums_dir.as_str(),
            ),
        ] {
            validate_relative_path(value, name)?;
        }
        for (name, value) in [
            (
                "frontend.aliases.implementation",
                self.frontend.aliases.implementation.as_str(),
            ),
            (
                "frontend.aliases.types",
                self.frontend.aliases.types.as_str(),
            ),
            (
                "frontend.aliases.enums",
                self.frontend.aliases.enums.as_str(),
            ),
        ] {
            if !value.starts_with('@') || value.contains('/') {
                bail!("{name} must be one bare @ alias");
            }
        }
        if self.frontend.request.module.trim().is_empty()
            || self.frontend.request.export.trim().is_empty()
        {
            bail!("frontend request module and export must not be empty");
        }
        validate_relative_path(&self.mock.output_root, "mock.outputRoot")?;
        for hook in &self.after_generate {
            if hook.command.trim().is_empty() {
                bail!("afterGenerate command must not be empty");
            }
        }
        if !self
            .frontend
            .response
            .code_fields
            .contains(&self.frontend.response.mock_code_field)
            || !self
                .frontend
                .response
                .data_fields
                .contains(&self.frontend.response.mock_data_field)
        {
            bail!("mock response fields must belong to detected response envelope fields");
        }
        Ok(())
    }

    pub fn validate_project(&self, project: &Path) -> Result<()> {
        self.validate()?;
        for relative in [
            self.frontend.build_tool.config_path.as_str(),
            self.frontend.tsconfig_path.as_str(),
        ] {
            if !project.join(relative).is_file() {
                bail!(
                    "nlab-api config drift: referenced file is missing: {}",
                    project.join(relative).display()
                );
            }
        }
        let build_source = fs::read_to_string(project.join(&self.frontend.build_tool.config_path))?;
        for alias in [
            &self.frontend.aliases.implementation,
            &self.frontend.aliases.types,
            &self.frontend.aliases.enums,
        ] {
            if !build_source.contains(&format!("'{alias}'"))
                && !build_source.contains(&format!("\"{alias}\""))
            {
                bail!("nlab-api config drift: build alias {alias} is missing; rerun init");
            }
        }
        for test_config in self
            .frontend
            .build_tool
            .test_configs
            .iter()
            .filter(|test_config| !test_config.inherits_build_config)
        {
            let source = fs::read_to_string(project.join(&test_config.path))?;
            for alias in [
                &self.frontend.aliases.implementation,
                &self.frontend.aliases.types,
                &self.frontend.aliases.enums,
            ] {
                if !source.contains(&format!("'{alias}'"))
                    && !source.contains(&format!("\"{alias}\""))
                {
                    bail!(
                        "nlab-api config drift: test alias {alias} is missing in {}; rerun init",
                        test_config.path
                    );
                }
            }
        }
        let module = self.frontend.request.module.as_str();
        if let Some(relative) = module.strip_prefix("@/") {
            let base = project.join(&self.frontend.source_root).join(relative);
            let found = [
                base.with_extension("ts"),
                base.with_extension("tsx"),
                base.join("index.ts"),
                base.join("index.tsx"),
            ]
            .into_iter()
            .any(|candidate| candidate.is_file());
            if !found {
                bail!(
                    "nlab-api config drift: request module {} is missing; rerun init",
                    self.frontend.request.module
                );
            }
        }
        Ok(())
    }
}

fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

fn read_json<T>(path: &Path, label: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("refuse to read symlinked {label}: {}", path.display());
    }
    let source =
        fs::read_to_string(path).with_context(|| format!("read {label} {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("decode {label} {}", path.display()))
}

fn regular_file_exists(path: &Path, label: &str) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            bail!("{label} must be a regular file: {}", path.display())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn reject_symlink_path(project: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(project)
        .with_context(|| format!("local nlab-api path is outside project: {}", path.display()))?;
    let mut current = project.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "refuse to write through symlinked path: {}",
                    current.display()
                )
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("local nlab-api path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("stage {}", path.display()))?;
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

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn validate_relative_path(value: &str, name: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        bail!("{name} must be a safe relative path");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_rejects_path_escape_and_non_alias() {
        assert!(validate_relative_path("src/service", "path").is_ok());
        assert!(validate_relative_path("../service", "path").is_err());

        let mut config = ProjectConfig {
            version: CONFIG_VERSION,
            backend: BackendConfig {
                repository: Some("git@example.com:team/backend.git".to_owned()),
                repo_path: PathBuf::from("/backend"),
                branch: "feature".to_owned(),
                app_name: "app".to_owned(),
                contract_roots: vec!["contract/src/main/java/p/contract".to_owned()],
            },
            frontend: FrontendConfig {
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
                    code_fields: vec!["code".to_owned()],
                    data_fields: vec!["data".to_owned()],
                    mock_code_field: "code".to_owned(),
                    mock_data_field: "data".to_owned(),
                },
                layout: OutputLayout {
                    preset: LayoutPreset::Service,
                    implementation_dir: "src/service".to_owned(),
                    types_dir: "src/types/service-types".to_owned(),
                    enums_dir: "src/types/service-enums".to_owned(),
                },
                aliases: ImportAliases {
                    implementation: "@service".to_owned(),
                    types: "@service-types".to_owned(),
                    enums: "@service-enums".to_owned(),
                },
            },
            gateway: Default::default(),
            migration: Default::default(),
            mock: Default::default(),
            after_generate: vec![],
        };
        assert!(config.validate().is_ok());
        let shared_source = config.shared_source().unwrap();
        let shared = serde_json::from_str::<serde_json::Value>(&shared_source).unwrap();
        assert_eq!(shared["version"], CONFIG_VERSION);
        assert_eq!(
            shared["backend"]["repository"],
            "git@example.com:team/backend.git"
        );
        assert!(shared["backend"].get("repoPath").is_none());

        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join(STATE_DIR)).unwrap();
        fs::write(project.path().join(CONFIG_FILE), &shared_source).unwrap();
        let local = LocalProjectConfig {
            backend: LocalBackendConfig {
                repo_path: Some(PathBuf::from("/local/backend")),
            },
            ..LocalProjectConfig::default()
        };
        local.save(project.path()).unwrap();
        assert_eq!(
            ProjectConfig::load(project.path())
                .unwrap()
                .backend
                .repo_path,
            PathBuf::from("/local/backend")
        );
        fs::remove_file(project.path().join(LOCAL_CONFIG_FILE)).unwrap();

        let mut version_one = serde_json::to_value(&config).unwrap();
        version_one["version"] = serde_json::json!(1);
        version_one["backend"]
            .as_object_mut()
            .unwrap()
            .remove("repository");
        fs::write(
            project.path().join(CONFIG_FILE),
            serde_json::to_vec_pretty(&version_one).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ProjectConfig::load(project.path())
                .unwrap()
                .backend
                .repo_path,
            PathBuf::from("/backend")
        );

        let mut legacy = serde_json::to_value(&config).unwrap();
        let backend = legacy["backend"].as_object_mut().unwrap();
        backend.remove("contractRoots");
        backend.insert(
            "contractRoot".to_owned(),
            serde_json::json!("contract/src/main/java/p/contract"),
        );
        let decoded = serde_json::from_value::<ProjectConfig>(legacy).unwrap();
        assert_eq!(decoded.backend.contract_roots.len(), 1);
        let encoded = serde_json::to_value(decoded).unwrap();
        assert!(encoded["backend"].get("contractRoot").is_none());
        assert!(encoded["backend"]["contractRoots"].is_array());
        config.frontend.aliases.types = "service-types".to_owned();
        assert!(config.validate().is_err());
        config.frontend.aliases.types = "@service-types".to_owned();
        config.after_generate = vec![AfterGenerateHook {
            command: "pnpm".to_owned(),
            args: vec!["exec".to_owned(), "eslint".to_owned(), "--fix".to_owned()],
            include_generated_files: true,
        }];
        assert!(config.validate().is_ok());
        config.after_generate[0].command.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn local_config_migrates_legacy_runner_and_preserves_backend_path() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join(STATE_DIR)).unwrap();
        fs::write(
            project.path().join(LEGACY_LOCAL_CONFIG_FILE),
            r#"{"version":1,"runner":"jt"}"#,
        )
        .unwrap();

        let mut local = LocalProjectConfig::load(project.path()).unwrap();
        assert_eq!(local.runner, Some(LocalRunner::Jt));
        local.backend.repo_path = Some(project.path().join("backend"));
        local.save(project.path()).unwrap();
        ensure_local_config_ignored(project.path()).unwrap();
        remove_legacy_local_config(project.path()).unwrap();

        let mut local = LocalProjectConfig::load(project.path()).unwrap();
        assert_eq!(local.runner, Some(LocalRunner::Jt));
        assert_eq!(
            local.backend.repo_path,
            Some(project.path().join("backend"))
        );
        local.runner = None;
        local.save(project.path()).unwrap();
        let reloaded = LocalProjectConfig::load(project.path()).unwrap();
        assert_eq!(reloaded.runner, None);
        assert_eq!(
            reloaded.backend.repo_path,
            Some(project.path().join("backend"))
        );
        assert_eq!(
            fs::read_to_string(project.path().join(".gitignore")).unwrap(),
            format!("{LOCAL_GITIGNORE_ENTRY}\n")
        );
    }

    #[test]
    fn config_command_supports_both_runners_show_and_unset() {
        let project = tempfile::tempdir().unwrap();
        let backend = project.path().join("backend");
        let mut local = LocalProjectConfig::default();
        local.backend.repo_path = Some(backend.clone());
        local.save(project.path()).unwrap();

        let configured = configure_inner(ConfigArgs {
            project: project.path().to_path_buf(),
            runner: Some(LocalRunner::NlabApi),
            unset: false,
            show: false,
            detect: false,
        })
        .unwrap();
        assert!(configured.contains("configured as nlab-api"));
        let shown = configure_inner(ConfigArgs {
            project: project.path().to_path_buf(),
            runner: None,
            unset: false,
            show: true,
            detect: false,
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&shown).unwrap()["runner"],
            "nlab-api"
        );

        configure_inner(ConfigArgs {
            project: project.path().to_path_buf(),
            runner: Some(LocalRunner::Jt),
            unset: false,
            show: false,
            detect: false,
        })
        .unwrap();
        assert_eq!(
            LocalProjectConfig::load(project.path()).unwrap().runner,
            Some(LocalRunner::Jt)
        );

        configure_inner(ConfigArgs {
            project: project.path().to_path_buf(),
            runner: None,
            unset: true,
            show: false,
            detect: false,
        })
        .unwrap();
        let local = LocalProjectConfig::load(project.path()).unwrap();
        assert_eq!(local.runner, None);
        assert_eq!(local.backend.repo_path, Some(backend));
    }

    #[test]
    fn config_detect_persists_jt_first_and_never_replaces_existing_runner() {
        let project = tempfile::tempdir().unwrap();
        let detect = || ConfigArgs {
            project: project.path().to_path_buf(),
            runner: None,
            unset: false,
            show: false,
            detect: true,
        };

        configure_inner_with(detect(), |_| true).unwrap();
        assert_eq!(
            LocalProjectConfig::load(project.path()).unwrap().runner,
            Some(LocalRunner::Jt)
        );

        configure_inner_with(detect(), |runner| runner == LocalRunner::NlabApi).unwrap();
        assert_eq!(
            LocalProjectConfig::load(project.path()).unwrap().runner,
            Some(LocalRunner::Jt)
        );

        let fallback = tempfile::tempdir().unwrap();
        configure_inner_with(
            ConfigArgs {
                project: fallback.path().to_path_buf(),
                runner: None,
                unset: false,
                show: false,
                detect: true,
            },
            |runner| runner == LocalRunner::NlabApi,
        )
        .unwrap();
        assert_eq!(
            LocalProjectConfig::load(fallback.path()).unwrap().runner,
            Some(LocalRunner::NlabApi)
        );

        let missing = tempfile::tempdir().unwrap();
        let error = configure_inner_with(
            ConfigArgs {
                project: missing.path().to_path_buf(),
                runner: None,
                unset: false,
                show: false,
                detect: true,
            },
            |_| false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("neither jt nor nlab-api"), "{error}");
        assert!(!missing.path().join(".nlab").exists());
    }
}
