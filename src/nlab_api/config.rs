use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Deserializer, Serialize};

pub const STATE_DIR: &str = ".nlab";
pub const CONFIG_FILE: &str = ".nlab/nlab-api.config.json";
pub const LEGACY_CONFIG_FILE: &str = "nlab-api.config.json";
pub const CONFIG_VERSION: u8 = 1;

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
    pub repo_path: PathBuf,
    pub branch: String,
    pub app_name: String,
    #[serde(
        alias = "contractRoot",
        deserialize_with = "deserialize_contract_roots"
    )]
    pub contract_roots: Vec<String>,
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
        let config = serde_json::from_str::<Self>(&source)
            .with_context(|| format!("decode nlab-api config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            bail!(
                "unsupported nlab-api config version {}; expected {CONFIG_VERSION}",
                self.version
            );
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
        };
        assert!(config.validate().is_ok());
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
    }
}
