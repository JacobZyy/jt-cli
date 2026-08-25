use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::node::{
    command::CommandSpec,
    context::AppContext,
    fs::{atomic_write, read_optional, remove_dir_all_safe, remove_file_safe},
    model::{CleanupAction, CleanupTarget, FormulaFact, PnpmProvider, StageOutcome},
    platform::first_executable,
    shell::{
        LegacyCleanupScope, reconcile_vite_loader, remove_legacy_toolchain_lines_scoped,
        shell_config_plan,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellCleanup {
    pub path: PathBuf,
    pub(crate) expected: Vec<u8>,
    pub(crate) content: Vec<u8>,
}

pub fn provider_evidence(provider: &PnpmProvider) -> String {
    let mut facts = vec![provider.detail.clone()];
    if let Some(real_path) = &provider.real_path {
        facts.push(format!("realpath={}", real_path.display()));
    }
    if let Some(version) = &provider.version {
        facts.push(format!("version={version}"));
    }
    if let Some(node) = &provider.node_version {
        facts.push(format!("node={node}"));
    }
    if let Some(pnpx) = &provider.pnpx_path {
        facts.push(format!("pnpx={}", pnpx.display()));
    }
    facts.join("; ")
}

pub fn safe_pnpm_home(home: &Path, pnpm_home: &Path) -> bool {
    pnpm_home.is_absolute()
        && pnpm_home != home
        && pnpm_home.starts_with(home)
        && pnpm_home
            .file_name()
            .is_some_and(|name| name.to_string_lossy().to_ascii_lowercase().contains("pnpm"))
}

pub fn relevant_homebrew_targets(formulas: &[FormulaFact]) -> Vec<CleanupTarget> {
    formulas
        .iter()
        .filter(|formula| {
            matches!(formula.name.as_str(), "pnpm" | "fnm" | "nvm" | "node")
                || formula.name.starts_with("node@")
                || !formula.relevant_files.is_empty()
        })
        .map(|formula| {
            let mut evidence = if formula.installed_dependents.is_empty() {
                "no installed Homebrew dependents reported".to_owned()
            } else {
                format!(
                    "installed dependents: {}",
                    formula.installed_dependents.join(", ")
                )
            };
            if !formula.relevant_files.is_empty() {
                evidence.push_str(&format!(
                    "; provides: {}",
                    formula.relevant_files.join(", ")
                ));
            }
            CleanupTarget {
                label: match &formula.version {
                    Some(version) => format!("Homebrew {} {version}", formula.name),
                    None => format!("Homebrew {}", formula.name),
                },
                action: CleanupAction::RemoveHomebrewFormula(formula.name.clone()),
                evidence,
                affected_packages: Vec::new(),
            }
        })
        .collect()
}

pub fn execute_targets(context: &AppContext<'_>, targets: &[CleanupTarget]) -> StageOutcome {
    let mut outcome = StageOutcome::default();
    for target in targets {
        match &target.action {
            CleanupAction::ReportOnly => {
                outcome.failure(format!("未清理 {}：{}", target.label, target.evidence))
            }
            CleanupAction::RemoveHomebrewFormula(formula) => {
                if let Err(error) = uninstall_homebrew(context, formula) {
                    outcome.failure(format!("卸载 Homebrew {formula} 失败：{error}"));
                } else {
                    outcome.note(format!("已卸载 Homebrew {formula}"));
                }
            }
            CleanupAction::RemovePnpmHome(path) => {
                match cleanup_pnpm_home(&context.home.home, path) {
                    Err(error) => outcome.failure(format!("清理 {} 失败：{error}", path.display())),
                    Ok(result) if result.unknown.is_empty() => {
                        outcome.note(format!(
                            "已清理 PNPM_HOME {} 的 globals/launcher",
                            path.display()
                        ));
                        if !result.preserved.is_empty() {
                            outcome.note(format!(
                                "已保留 PNPM_HOME 通用数据：{}",
                                result.preserved.join(", ")
                            ));
                        }
                    }
                    Ok(result) => {
                        outcome.failure(format!(
                            "PNPM_HOME {} 已清理已知 globals，但保留未知内容：{}",
                            path.display(),
                            result.unknown.join(", ")
                        ));
                        if !result.preserved.is_empty() {
                            outcome.note(format!(
                                "已保留 PNPM_HOME 通用数据：{}",
                                result.preserved.join(", ")
                            ));
                        }
                    }
                }
            }
        }
    }
    outcome
}

pub fn preview_shell_cleanup(
    context: &AppContext<'_>,
    scope: &LegacyCleanupScope<'_>,
) -> (Vec<ShellCleanup>, Vec<String>) {
    let mut changes = Vec::new();
    let mut diagnostics = Vec::new();
    for (shell, path, enabled) in shell_config_plan(&context.home.home, &context.environment) {
        let current = match read_optional(&path) {
            Ok(current) => current,
            Err(error) => {
                diagnostics.push(format!("无法读取 shell 配置 {}：{error}", path.display()));
                continue;
            }
        };
        let Some(current) = current else { continue };
        let source = match std::str::from_utf8(&current) {
            Ok(source) => source,
            Err(error) => {
                diagnostics.push(format!(
                    "shell 配置不是 UTF-8，拒绝重写 {}：{error}",
                    path.display()
                ));
                continue;
            }
        };
        let cleaned = remove_legacy_toolchain_lines_scoped(source, shell, scope);
        let next = reconcile_vite_loader(&cleaned, shell, enabled);
        if next.as_bytes() != current.as_slice() {
            changes.push(ShellCleanup {
                path,
                expected: current,
                content: next.into_bytes(),
            });
        }
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    changes.dedup_by(|left, right| left.path == right.path);
    (changes, diagnostics)
}

pub fn cleanup_shell_configuration(
    context: &AppContext<'_>,
    changes: &[ShellCleanup],
) -> StageOutcome {
    let mut outcome = StageOutcome::default();
    for change in changes {
        match atomic_write(
            &context.home.home,
            &change.path,
            Some(&change.expected),
            &change.content,
        ) {
            Ok(()) => outcome.note(format!("已清理 shell 配置 {}", change.path.display())),
            Err(error) => outcome.failure(format!(
                "更新 shell 配置 {} 失败：{error}",
                change.path.display()
            )),
        }
    }
    outcome
}

pub fn remove_nvm_root(home: &Path, root: &Path) -> Result<(), String> {
    if root != home.join(".nvm")
        || !root.join("nvm.sh").is_file()
        || !root.join("versions/node").is_dir()
    {
        return Err("not a verified nvm root".to_owned());
    }
    remove_dir_all_safe(home, root).map_err(|error| error.to_string())
}

pub fn remove_fnm_data_root(home: &Path, root: &Path) -> Result<(), String> {
    let known = [
        home.join(".fnm"),
        home.join(".local/share/fnm"),
        home.join("Library/Application Support/fnm"),
    ];
    if !known.contains(&root.to_path_buf()) || !root.join("node-versions").is_dir() {
        return Err("not a verified fnm data root".to_owned());
    }
    remove_dir_all_safe(home, root).map_err(|error| error.to_string())
}

pub fn remove_fnm_multishell_root(home: &Path, root: &Path) -> Result<(), String> {
    let known = [
        home.join(".local/state/fnm_multishells"),
        home.join("Library/Caches/fnm_multishells"),
    ];
    if !known.contains(&root.to_path_buf()) {
        return Err("not a verified fnm multishell root".to_owned());
    }
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err("not a verified fnm multishell root".to_owned());
        }
        Ok(_) => {}
    }
    remove_dir_all_safe(home, root).map_err(|error| error.to_string())
}

pub fn merge_outcome(into: &mut StageOutcome, next: StageOutcome) {
    into.completed.extend(next.completed);
    into.failures.extend(next.failures);
    into.incomplete |= next.incomplete;
}

#[derive(Debug, Eq, PartialEq)]
struct PnpmHomeCleanup {
    preserved: Vec<String>,
    unknown: Vec<String>,
}

fn cleanup_pnpm_home(home: &Path, pnpm_home: &Path) -> Result<PnpmHomeCleanup, String> {
    if !safe_pnpm_home(home, pnpm_home) {
        return Err(format!(
            "refuse broad or unverified PNPM_HOME: {}",
            pnpm_home.display()
        ));
    }
    let result = inspect_pnpm_home_contents(pnpm_home)?;
    let candidates = vec![
        pnpm_home.join("global"),
        pnpm_home.join("pnpm"),
        pnpm_home.join("pnpx"),
        pnpm_home.join("bin/pnpm"),
        pnpm_home.join("bin/pnpx"),
        pnpm_home.join("bin/pn"),
        pnpm_home.join("bin/pnx"),
    ];
    if !candidates
        .iter()
        .any(|path| fs::symlink_metadata(path).is_ok())
    {
        return Err("PNPM_HOME lacks verified global layout".to_owned());
    }
    for path in candidates {
        if fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.is_dir() {
            remove_dir_all_safe(home, &path).map_err(|error| error.to_string())?;
        } else {
            remove_file_safe(home, &path).map_err(|error| error.to_string())?;
        }
    }
    let bin = pnpm_home.join("bin");
    if fs::symlink_metadata(&bin).is_ok_and(|metadata| metadata.is_dir())
        && fs::read_dir(&bin)
            .map_err(|error| error.to_string())?
            .next()
            .is_none()
    {
        fs::remove_dir(&bin).map_err(|error| error.to_string())?;
    }
    if result.preserved.is_empty() && result.unknown.is_empty() {
        remove_dir_all_safe(home, pnpm_home).map_err(|error| error.to_string())?;
    }
    Ok(result)
}

fn inspect_pnpm_home_contents(pnpm_home: &Path) -> Result<PnpmHomeCleanup, String> {
    let mut preserved = Vec::new();
    let mut unknown = Vec::new();
    let entries = fs::read_dir(pnpm_home).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if matches!(name.as_str(), "store" | "cache" | ".cache") && file_type.is_dir() {
            preserved.push(name);
        } else if name == "global" {
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err("PNPM_HOME global is not a verified directory".to_owned());
            }
        } else if matches!(name.as_str(), "pnpm" | "pnpx") {
            if !file_type.is_file() && !file_type.is_symlink() {
                return Err(format!("PNPM_HOME launcher has unsafe type: {name}"));
            }
        } else if name == "bin" && file_type.is_dir() {
            for child in fs::read_dir(entry.path()).map_err(|error| error.to_string())? {
                let child = child.map_err(|error| error.to_string())?;
                let child_name = child.file_name().to_string_lossy().into_owned();
                let child_type = child.file_type().map_err(|error| error.to_string())?;
                if matches!(child_name.as_str(), "pnpm" | "pnpx" | "pn" | "pnx")
                    && !child_type.is_file()
                    && !child_type.is_symlink()
                {
                    return Err(format!(
                        "PNPM_HOME bin launcher has unsafe type: {child_name}"
                    ));
                }
                if !matches!(child_name.as_str(), "pnpm" | "pnpx" | "pn" | "pnx") {
                    unknown.push(format!("bin/{child_name}"));
                }
            }
        } else {
            unknown.push(name);
        }
    }
    preserved.sort();
    unknown.sort();
    Ok(PnpmHomeCleanup { preserved, unknown })
}

fn uninstall_homebrew(context: &AppContext<'_>, formula: &str) -> crate::node::error::Result<()> {
    let brew = first_executable("brew", &context.environment).ok_or_else(|| {
        crate::node::error::AppError::Invalid("Homebrew is unavailable".to_owned())
    })?;
    let result = context
        .runner
        .run(&CommandSpec::new(brew, ["uninstall", formula]))?;
    result.require_success(&format!("brew uninstall {formula}"), &[])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::node::model::{FormulaFact, PnpmProvider, PnpmProviderKind};

    use super::{cleanup_pnpm_home, provider_evidence, relevant_homebrew_targets, safe_pnpm_home};

    #[test]
    fn pnpm_cleanup_preserves_unknown_content_and_generic_store() {
        let home = tempdir().unwrap();
        let pnpm = home.path().join("Library/pnpm");
        fs::create_dir_all(pnpm.join("global/5")).unwrap();
        fs::write(pnpm.join("pnpm"), "launcher").unwrap();
        fs::create_dir_all(pnpm.join("store")).unwrap();
        fs::write(pnpm.join("keep-me.txt"), "unknown").unwrap();

        let result = cleanup_pnpm_home(home.path(), &pnpm).unwrap();

        assert!(pnpm.join("store").is_dir());
        assert!(!pnpm.join("global").exists());
        assert!(!pnpm.join("pnpm").exists());
        assert_eq!(result.preserved, ["store"]);
        assert_eq!(result.unknown, ["keep-me.txt"]);
    }

    #[test]
    fn pnpm_cleanup_removes_bin_launchers_and_store_is_not_unknown() {
        let home = tempdir().unwrap();
        let pnpm = home.path().join("Library/pnpm");
        fs::create_dir_all(pnpm.join("global/11")).unwrap();
        fs::create_dir_all(pnpm.join("bin")).unwrap();
        fs::create_dir_all(pnpm.join("store/v11")).unwrap();
        for launcher in ["pnpm", "pnpx", "pn", "pnx"] {
            fs::write(pnpm.join("bin").join(launcher), "launcher").unwrap();
        }

        let result = cleanup_pnpm_home(home.path(), &pnpm).unwrap();

        assert_eq!(result.preserved, ["store"]);
        assert!(result.unknown.is_empty());
        assert!(!pnpm.join("bin").exists());
        assert!(pnpm.join("store").is_dir());
    }

    #[test]
    fn pnpm_cleanup_rejects_broad_home_paths() {
        let home = tempdir().unwrap();

        assert!(!safe_pnpm_home(home.path(), home.path()));
        assert!(!safe_pnpm_home(home.path(), &home.path().join(".local")));
        assert!(safe_pnpm_home(
            home.path(),
            &home.path().join(".local/share/pnpm")
        ));
    }

    #[test]
    fn multishell_cleanup_is_idempotent_after_parent_was_removed() {
        let home = tempdir().unwrap();
        let root = home.path().join(".local/state/fnm_multishells");
        fs::create_dir_all(root.join("session/bin")).unwrap();

        super::remove_fnm_multishell_root(home.path(), &root).unwrap();
        super::remove_fnm_multishell_root(home.path(), &root).unwrap();

        assert!(!root.exists());
    }

    #[test]
    fn fnm_cleanup_rejects_broad_custom_root() {
        let home = tempdir().unwrap();
        let broad = home.path().join(".local/share");
        fs::create_dir_all(broad.join("node-versions")).unwrap();

        assert!(super::remove_fnm_data_root(home.path(), &broad).is_err());
        assert!(broad.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn multishell_cleanup_rejects_a_leaf_symlink() {
        use std::os::unix::fs::symlink;

        let home = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let root = home.path().join(".local/state/fnm_multishells");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        symlink(outside.path(), &root).unwrap();

        assert!(super::remove_fnm_multishell_root(home.path(), &root).is_err());
        assert!(outside.path().is_dir());
    }

    #[test]
    fn multishell_cleanup_rejects_custom_named_root() {
        let home = tempdir().unwrap();
        let root = home.path().join("Documents/fnm_multishells");
        fs::create_dir_all(root.join("session")).unwrap();

        assert!(super::remove_fnm_multishell_root(home.path(), &root).is_err());
        assert!(root.is_dir());
    }

    #[test]
    fn formula_with_verified_toolchain_binary_is_a_cleanup_target() {
        let targets = relevant_homebrew_targets(&[FormulaFact {
            name: "company-node-tools".to_owned(),
            version: Some("1.2.3".to_owned()),
            prefix: None,
            installed_dependents: vec!["consumer".to_owned()],
            relevant_files: vec![
                "/opt/homebrew/Cellar/company-node-tools/1.2.3/bin/pnpm".to_owned(),
            ],
        }]);

        assert_eq!(targets.len(), 1);
        assert!(targets[0].evidence.contains("consumer"));
        assert!(targets[0].evidence.contains("/bin/pnpm"));
    }

    #[test]
    fn provider_evidence_carries_realpath_version_and_node() {
        let mut fact = provider(PnpmProviderKind::Standalone, Some("/home/a/pnpm"));
        fact.real_path = Some("/real/pnpm".into());
        fact.version = Some("10.12.4".to_owned());
        fact.node_version = Some("24.15.0".to_owned());
        fact.pnpx_path = Some("/home/a/pnpx".into());

        let evidence = provider_evidence(&fact);

        assert!(evidence.contains("realpath=/real/pnpm"));
        assert!(evidence.contains("version=10.12.4"));
        assert!(evidence.contains("node=24.15.0"));
        assert!(evidence.contains("pnpx=/home/a/pnpx"));
    }

    fn provider(kind: PnpmProviderKind, prefix: Option<&str>) -> PnpmProvider {
        PnpmProvider {
            kind,
            pnpm_path: "/bin/pnpm".into(),
            pnpx_path: None,
            real_path: None,
            version: None,
            node_version: None,
            prefix: prefix.map(Into::into),
            globals: Vec::new(),
            detail: "test".to_owned(),
        }
    }
}
