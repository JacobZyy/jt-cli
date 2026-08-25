use std::{fmt, path::PathBuf};

pub const PACKAGE_REGISTRY: &str = "https://registry.npmmirror.com/";
pub const NODE_MIRROR: &str = "https://npmmirror.com/mirrors/node/";
pub const ZZ_REGISTRY: &str = "https://rcnpm.zhuanspirit.com/";
pub const VITE_PLUS_HOME_DIR: &str = ".vite-plus";
pub const VITE_PLUS_INSTALLER_URL: &str = "https://vite.plus";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Toolchain {
    pub node: &'static str,
    pub pnpm: &'static str,
}

pub const TOOLCHAINS: [Toolchain; 5] = [
    Toolchain {
        node: "14.21.3",
        pnpm: "7.4.1",
    },
    Toolchain {
        node: "16.19.0",
        pnpm: "7.4.1",
    },
    Toolchain {
        node: "20.11.0",
        pnpm: "9.7.0",
    },
    Toolchain {
        node: "22.21.0",
        pnpm: "10.12.4",
    },
    Toolchain {
        node: "24.15.0",
        pnpm: "10.12.4",
    },
];

pub const PNPM_VERSIONS: [&str; 3] = ["7.4.1", "9.7.0", "10.12.4"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ManagerKind {
    Nvm,
    Fnm,
}

impl fmt::Display for ManagerKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Nvm => "nvm",
            Self::Fnm => "fnm",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PnpmProviderKind {
    Corepack,
    NpmGlobal,
    Standalone,
    Homebrew,
    Unknown,
}

impl fmt::Display for PnpmProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Corepack => "Corepack",
            Self::NpmGlobal => "npm global",
            Self::Standalone => "standalone / PNPM_HOME",
            Self::Homebrew => "Homebrew",
            Self::Unknown => "unknown",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagerFact {
    pub kind: ManagerKind,
    pub executable: Option<PathBuf>,
    pub root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeFact {
    pub manager: Option<ManagerKind>,
    pub provider: String,
    pub version: String,
    pub root: PathBuf,
    pub node_path: PathBuf,
    pub npm_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormulaFact {
    pub name: String,
    pub version: Option<String>,
    pub prefix: Option<PathBuf>,
    pub installed_dependents: Vec<String>,
    pub relevant_files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PnpmProvider {
    pub kind: PnpmProviderKind,
    pub pnpm_path: PathBuf,
    pub pnpx_path: Option<PathBuf>,
    pub real_path: Option<PathBuf>,
    pub version: Option<String>,
    pub node_version: Option<String>,
    pub prefix: Option<PathBuf>,
    pub globals: Vec<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceFact {
    pub source: String,
    pub location: String,
    pub excerpt: String,
    pub impact: ReferenceImpact,
    pub evidence: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceImpact {
    Affected,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CleanupAction {
    RemoveHomebrewFormula(String),
    RemovePnpmHome(PathBuf),
    ReportOnly,
}

impl fmt::Display for CleanupAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RemoveHomebrewFormula(formula) => write!(formatter, "brew uninstall {formula}"),
            Self::RemovePnpmHome(path) => write!(
                formatter,
                "清理 {} 的已验证 globals/launcher（保留 store/cache）",
                path.display()
            ),
            Self::ReportOnly => formatter.write_str("仅报告，不删除"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupTarget {
    pub label: String,
    pub action: CleanupAction,
    pub evidence: String,
    pub affected_packages: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackageSource {
    Registry,
    Git,
    File,
    Link,
    Workspace,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalPackage {
    pub name: String,
    pub version: String,
    pub source: PackageSource,
    pub provider: String,
    pub node_version: Option<String>,
    pub bins: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalCandidate {
    pub name: String,
    pub version: String,
    pub origins: Vec<GlobalPackage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalStatus {
    Installed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageStatus {
    Completed,
    Failed,
    Skipped,
    Partial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageReport {
    pub name: String,
    pub status: StageStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalResult {
    pub name: String,
    pub expected_version: String,
    pub status: GlobalStatus,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StageOutcome {
    pub completed: Vec<String>,
    pub failures: Vec<String>,
    pub incomplete: bool,
}

impl StageOutcome {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            completed: vec![message.into()],
            ..Self::default()
        }
    }

    pub fn note(&mut self, message: impl Into<String>) {
        self.completed.push(message.into());
    }

    pub fn failure(&mut self, message: impl Into<String>) {
        self.failures.push(message.into());
        self.incomplete = true;
    }
}
