use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use tempfile::{Builder, TempDir};

use crate::node::{
    cleanup::{
        cleanup_shell_configuration, execute_targets, merge_outcome, preview_shell_cleanup,
        provider_evidence, relevant_homebrew_targets, remove_fnm_data_root,
        remove_fnm_multishell_root, remove_nvm_root, safe_pnpm_home,
    },
    command::{CommandResult, CommandSpec, inherited_proxy_env, redact},
    context::AppContext,
    error::{AppError, Result},
    fs::{atomic_write, ensure_safe_home_target, read_optional, remove_file_safe},
    inventory::{
        GlobalInventory, detect_references, inspect_environment, inventory_legacy_globals,
    },
    model::{
        CleanupAction, CleanupTarget, GlobalCandidate, GlobalResult, GlobalStatus, ManagerKind,
        NODE_MIRROR, PACKAGE_REGISTRY, PNPM_VERSIONS, ReferenceFact, StageOutcome, TOOLCHAINS,
        VITE_PLUS_INSTALLER_URL,
    },
    nrm::configure_taobao_and_zz,
    platform::{executable_candidates, first_executable},
    shell::{reconcile_vite_loader, shell_config_plan, validate_zdotdir},
};

#[derive(Clone, Debug)]
pub struct VitePlusInstall {
    pub vp: PathBuf,
    pub home: PathBuf,
    pub version: String,
    pub default_node: String,
    pub default_pnpm: String,
}

#[derive(Clone, Debug)]
pub struct GlobalStage {
    pub inventory: GlobalInventory,
    pub results: Vec<GlobalResult>,
}

#[derive(Clone, Debug, Default)]
pub struct AggressiveCleanupPlan {
    pub targets: Vec<CleanupTarget>,
    pub shell_changes: Vec<crate::node::cleanup::ShellCleanup>,
    pub nvm_roots: Vec<PathBuf>,
    pub fnm_data_roots: Vec<PathBuf>,
    pub fnm_multishell_roots: Vec<PathBuf>,
    pub cargo_fnm: bool,
    pub references: Vec<ReferenceFact>,
    pub unaffected_reference_count: usize,
    pub global_failures: Vec<GlobalResult>,
    pub unreconstructable_globals: Vec<crate::node::model::GlobalPackage>,
    pub global_fingerprint: Vec<String>,
    pub runtime_fingerprint: Vec<String>,
    pub diagnostics: Vec<String>,
}

impl AggressiveCleanupPlan {
    pub fn same_actions(&self, other: &Self) -> bool {
        self.targets == other.targets
            && self.shell_changes == other.shell_changes
            && self.nvm_roots == other.nvm_roots
            && self.fnm_data_roots == other.fnm_data_roots
            && self.fnm_multishell_roots == other.fnm_multishell_roots
            && self.cargo_fnm == other.cargo_fnm
            && self.global_fingerprint == other.global_fingerprint
            && self.runtime_fingerprint == other.runtime_fingerprint
    }
}

pub fn install(context: &AppContext<'_>) -> Result<VitePlusInstall> {
    let home = context.home.vite_plus_home();
    let vp = home.join("bin/vp");
    let environment = vite_environment(context, &home);
    let version = progress_step(context, 1, "准备 Vite+", || {
        match usable_vp(context, &vp, &environment) {
            Some(version) => Ok(version),
            None => install_vp(context, &home, &environment),
        }
    })?;

    let bootstrap = TempDir::new_in(&context.home.temp_root)
        .map_err(|error| AppError::io("create Vite+ bootstrap directory", None, error))?;
    let node = home.join("bin/node");
    let default_node = progress_step(context, 2, "初始化 default Node", || {
        run_vp(
            context,
            &vp,
            &environment,
            bootstrap.path(),
            ["env", "setup"],
            "setup Vite+ default environment",
        )?;
        require_version(
            run_program(
                context,
                &node,
                &environment,
                Some(bootstrap.path()),
                ["--version"],
            )?,
            "verify Vite+ default Node",
        )
    })?;
    let pnpm = home.join("bin/pnpm");
    let default_pnpm = progress_step(context, 3, "安装 default pnpm", || {
        run_vp(
            context,
            &vp,
            &environment,
            bootstrap.path(),
            ["install", "-g", "pnpm"],
            "install Vite+ default pnpm",
        )?;
        require_version(
            run_program(
                context,
                &pnpm,
                &environment,
                Some(bootstrap.path()),
                ["--version"],
            )?,
            "verify Vite+ default pnpm",
        )
    })?;

    for (index, pair) in TOOLCHAINS.iter().enumerate() {
        progress_step(
            context,
            index + 4,
            &format!("预装 Node {}", pair.node),
            || {
                if std::env::consts::OS == "macos"
                    && std::env::consts::ARCH == "aarch64"
                    && pair.node == "14.21.3"
                {
                    install_node14_x64(
                        context,
                        &vp,
                        &home,
                        &environment,
                        &version,
                        bootstrap.path(),
                    )
                } else {
                    run_vp(
                        context,
                        &vp,
                        &environment,
                        bootstrap.path(),
                        ["env", "install", pair.node],
                        &format!("preinstall Node {}", pair.node),
                    )
                    .map(|_| ())
                }
            },
        )?;
    }
    for (index, version) in PNPM_VERSIONS.iter().enumerate() {
        progress_step(
            context,
            index + 9,
            &format!("预热 pnpm {version}"),
            || prewarm_pnpm(context, &pnpm, &environment, version),
        )?;
    }
    progress_step(context, 12, "安装 nrm", || {
        run_vp(
            context,
            &vp,
            &environment,
            bootstrap.path(),
            ["install", "-g", "nrm"],
            "install Vite+ nrm",
        )
        .map(|_| ())
    })?;
    Ok(VitePlusInstall {
        vp,
        home,
        version,
        default_node,
        default_pnpm,
    })
}

fn progress_step<T>(
    context: &AppContext<'_>,
    step: usize,
    message: &str,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let message = format!("[{step}/12] {message}");
    context.prompt.start_progress(&message)?;
    match run() {
        Ok(value) => {
            context
                .prompt
                .finish_progress(&format!("{message}：完成"))?;
            Ok(value)
        }
        Err(error) => {
            let _ = context.prompt.fail_progress(&format!("{message}：失败"));
            Err(error)
        }
    }
}

pub fn configure(context: &AppContext<'_>, install: &VitePlusInstall) -> Result<()> {
    validate_zdotdir(&context.home.home, &context.environment)?;
    let environment = vite_environment(context, &install.home);
    let mut snapshots = configuration_snapshots(context)?;
    let configured = (|| {
        configure_taobao_and_zz(
            context.runner,
            &install.home.join("bin/nrm"),
            &environment,
            &context.home.home.join(".nrmrc"),
        )?;
        configure_shell_blocks(context, &mut snapshots)
    })();
    if let Err(error) = configured {
        let rollback = match restore_configuration_snapshots(context, &snapshots) {
            Ok(()) => "shell 配置已按 expected-content guard 回滚".to_owned(),
            Err(rollback_error) => format!("shell 配置回滚失败：{rollback_error}"),
        };
        return Err(AppError::Invalid(format!(
            "{error}；{rollback}；nrm 外部状态未自动回滚"
        )));
    }
    Ok(())
}

pub fn configure_global_environment(
    context: &AppContext<'_>,
    install: &VitePlusInstall,
) -> Result<GlobalStage> {
    let environment_snapshot = inspect_environment(context);
    let inventory = inventory_legacy_globals(context, &environment_snapshot);
    let environment = vite_environment(context, &install.home);
    let fixture = TempDir::new_in(&context.home.temp_root)
        .map_err(|error| AppError::io("create Vite+ global fixture", None, error))?;
    let specs = inventory
        .candidates
        .iter()
        .map(|candidate| format!("{}@{}", candidate.name, candidate.version))
        .collect::<Vec<_>>();
    let bulk_error = if specs.is_empty() {
        None
    } else {
        let mut args = vec!["install".to_owned(), "-g".to_owned()];
        args.extend(specs);
        let result = run_vp_vec(context, &install.vp, &environment, fixture.path(), args)?;
        (!result.success()).then(|| command_detail(&result))
    };
    let listed = run_vp(
        context,
        &install.vp,
        &environment,
        fixture.path(),
        ["list", "-g", "--json"],
        "read Vite+ global packages",
    )?;
    let actual = decode_vp_globals(&listed.stdout)?;
    let results = global_results(&inventory.candidates, &actual, bulk_error.as_deref());
    Ok(GlobalStage { inventory, results })
}

pub fn verify_vp_global_candidates(
    context: &AppContext<'_>,
    install: &VitePlusInstall,
    candidates: &[GlobalCandidate],
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    let fixture = TempDir::new_in(&context.home.temp_root)
        .map_err(|error| AppError::io("create Vite+ global verification fixture", None, error))?;
    let listed = run_vp(
        context,
        &install.vp,
        &vite_environment(context, &install.home),
        fixture.path(),
        ["list", "-g", "--json"],
        "fresh read Vite+ global packages",
    )?;
    let actual = decode_vp_globals(&listed.stdout)?;
    let failures = global_results(candidates, &actual, None)
        .into_iter()
        .filter(|result| result.status != GlobalStatus::Installed)
        .map(|result| format!("{}@{}", result.name, result.expected_version))
        .collect::<Vec<_>>();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Invalid(format!(
            "Vite+ globals missing or changed after confirmation: {}",
            failures.join(", ")
        )))
    }
}

pub fn cleanup_plan(context: &AppContext<'_>, globals: &GlobalStage) -> AggressiveCleanupPlan {
    let environment = inspect_environment(context);
    let fresh_globals = inventory_legacy_globals(context, &environment);
    let mut targets = relevant_homebrew_targets(&environment.formulas);
    let mut retained_pnpm_provider = false;
    for provider in &environment.pnpm_providers {
        match provider.kind {
            crate::node::model::PnpmProviderKind::Standalone => {
                if let Some(home) = &provider.prefix {
                    let safe = safe_pnpm_home(&context.home.home, home);
                    let has_unproven_globals = fresh_globals
                        .unreconstructable
                        .iter()
                        .any(|package| package.provider == "pnpm-home");
                    let will_remove = safe && !has_unproven_globals;
                    retained_pnpm_provider |= !will_remove;
                    targets.push(CleanupTarget {
                        label: format!("PNPM_HOME {}", home.display()),
                        action: if will_remove {
                            CleanupAction::RemovePnpmHome(home.clone())
                        } else {
                            CleanupAction::ReportOnly
                        },
                        evidence: if !safe {
                            format!(
                                "{}; broad or unverified PNPM_HOME is never removed",
                                provider_evidence(provider)
                            )
                        } else if has_unproven_globals {
                            format!(
                                "{}; globals lack native registry provenance, so PNPM_HOME is retained",
                                provider_evidence(provider)
                            )
                        } else {
                            provider_evidence(provider)
                        },
                        affected_packages: provider.globals.clone(),
                    });
                }
            }
            crate::node::model::PnpmProviderKind::Unknown => {
                retained_pnpm_provider = true;
                targets.push(CleanupTarget {
                    label: format!("unknown pnpm {}", provider.pnpm_path.display()),
                    action: CleanupAction::ReportOnly,
                    evidence: provider_evidence(provider),
                    affected_packages: provider.globals.clone(),
                });
            }
            _ => {}
        }
    }
    if retained_pnpm_provider {
        for target in &mut targets {
            if matches!(target.action, CleanupAction::RemovePnpmHome(_)) {
                target.action = CleanupAction::ReportOnly;
                target.evidence.push_str(
                    "; another pnpm provider is retained, so all PNPM_HOME targets are retained",
                );
            }
        }
    }
    for package in fresh_globals
        .packages
        .iter()
        .filter(|package| package.provider == "bun")
    {
        targets.push(CleanupTarget {
            label: format!("Bun global {}", package.name),
            action: CleanupAction::ReportOnly,
            evidence:
                "Bun effective globalDir depends on runtime config; jt keeps it until target binding is provable"
                    .to_owned(),
            affected_packages: vec![package.name.clone()],
        });
    }
    let mut nvm_roots = Vec::new();
    let mut fnm_data_roots = Vec::new();
    let mut retained_nvm_root = false;
    let mut retained_fnm_root = false;
    for manager in &environment.managers {
        let safe = match manager.kind {
            ManagerKind::Nvm => safe_nvm_root(&context.home.home, &manager.root),
            ManagerKind::Fnm => safe_fnm_root(&context.home.home, &manager.root),
        };
        if safe {
            match manager.kind {
                ManagerKind::Nvm => nvm_roots.push(manager.root.clone()),
                ManagerKind::Fnm => fnm_data_roots.push(manager.root.clone()),
            }
        } else {
            match manager.kind {
                ManagerKind::Nvm => retained_nvm_root = true,
                ManagerKind::Fnm => retained_fnm_root = true,
            }
            targets.push(CleanupTarget {
                label: format!("custom {} root {}", manager.kind, manager.root.display()),
                action: CleanupAction::ReportOnly,
                evidence: "root is not an exact dedicated jt-known manager directory".to_owned(),
                affected_packages: fresh_globals
                    .packages
                    .iter()
                    .filter(|package| package.provider == manager.kind.to_string())
                    .map(|package| package.name.clone())
                    .collect(),
            });
        }
    }
    nvm_roots.sort();
    fnm_data_roots.sort();
    for target in &mut targets {
        let retained_manager = match &target.action {
            CleanupAction::RemoveHomebrewFormula(formula) if formula == "nvm" => retained_nvm_root,
            CleanupAction::RemoveHomebrewFormula(formula) if formula == "fnm" => retained_fnm_root,
            _ => false,
        };
        if retained_manager {
            target.action = CleanupAction::ReportOnly;
            target
                .evidence
                .push_str("; custom manager root is retained, so provider is retained");
        }
    }
    let cargo_fnm_detected = cargo_has_fnm(context);
    let cargo_fnm = cargo_fnm_detected && !retained_fnm_root;
    if cargo_fnm_detected && !cargo_fnm {
        targets.push(CleanupTarget {
            label: "Cargo fnm".to_owned(),
            action: CleanupAction::ReportOnly,
            evidence: "custom fnm root is retained, so Cargo fnm is retained".to_owned(),
            affected_packages: Vec::new(),
        });
    }
    let mut retained_fnm_launcher = false;
    for launcher in fnm_launchers(context) {
        let removed_with_root = fnm_data_roots.iter().any(|root| launcher.starts_with(root));
        let removed_by_cargo = cargo_fnm && launcher == context.home.home.join(".cargo/bin/fnm");
        if !removed_with_root && !removed_by_cargo {
            retained_fnm_launcher = true;
            targets.push(CleanupTarget {
                label: format!("unverified fnm launcher {}", launcher.display()),
                action: CleanupAction::ReportOnly,
                evidence: "launcher ownership is not provable; file is retained".to_owned(),
                affected_packages: Vec::new(),
            });
        }
    }
    let remove_nvm = !retained_nvm_root
        && (!nvm_roots.is_empty()
            || targets.iter().any(|target| {
                matches!(
                    &target.action,
                    CleanupAction::RemoveHomebrewFormula(formula) if formula == "nvm"
                )
            }));
    let remove_fnm = !retained_fnm_root
        && (!fnm_data_roots.is_empty()
            || targets.iter().any(|target| {
                matches!(
                    &target.action,
                    CleanupAction::RemoveHomebrewFormula(formula) if formula == "fnm"
                )
            }));
    let detected_fnm_multishell_roots = fnm_multishell_roots(context);
    let mut fnm_multishell_roots = Vec::new();
    for root in detected_fnm_multishell_roots {
        if remove_fnm && safe_fnm_multishell_root(&context.home.home, &root) {
            fnm_multishell_roots.push(root);
        } else {
            targets.push(CleanupTarget {
                label: format!("fnm multishell {}", root.display()),
                action: CleanupAction::ReportOnly,
                evidence: if remove_fnm {
                    "multishell root is not an exact dedicated jt-known directory".to_owned()
                } else {
                    "fnm provider is retained, so multishell data is retained".to_owned()
                },
                affected_packages: Vec::new(),
            });
        }
    }
    deduplicate_cleanup_targets(&mut targets);
    let pnpm_shell_roots = targets
        .iter()
        .filter_map(|target| match &target.action {
            CleanupAction::RemovePnpmHome(path) => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let remove_pnpm = !pnpm_shell_roots.is_empty();
    let nvm_shell_roots = if remove_nvm {
        vec![context.home.home.join(".nvm")]
    } else {
        Vec::new()
    };
    let fnm_shell_roots = if remove_fnm {
        vec![
            context.home.home.join(".fnm"),
            context.home.home.join(".local/share/fnm"),
            context.home.home.join("Library/Application Support/fnm"),
        ]
    } else {
        Vec::new()
    };
    let fnm_multishell_shell_roots = if remove_fnm {
        vec![
            context.home.home.join(".local/state/fnm_multishells"),
            context.home.home.join("Library/Caches/fnm_multishells"),
        ]
    } else {
        Vec::new()
    };
    let shell_scope = crate::node::shell::LegacyCleanupScope {
        home: &context.home.home,
        nvm_roots: &nvm_shell_roots,
        fnm_roots: &fnm_shell_roots,
        fnm_multishell_roots: &fnm_multishell_shell_roots,
        pnpm_roots: &pnpm_shell_roots,
        remove_manager_block: remove_nvm && remove_fnm && !retained_fnm_launcher,
    };
    let (shell_changes, shell_diagnostics) = preview_shell_cleanup(context, &shell_scope);
    let mut reference_roots = nvm_roots
        .iter()
        .chain(&fnm_data_roots)
        .chain(&fnm_multishell_roots)
        .cloned()
        .collect::<Vec<_>>();
    if cargo_fnm {
        reference_roots.push(context.home.home.join(".cargo/bin/fnm"));
    }
    for target in &targets {
        if let CleanupAction::RemovePnpmHome(path) = &target.action {
            reference_roots.push(path.clone());
        }
    }
    for formula in environment.formulas.iter().filter(|formula| {
        targets.iter().any(|target| {
            matches!(
                &target.action,
                CleanupAction::RemoveHomebrewFormula(name) if name == &formula.name
            )
        })
    }) {
        if let Some(prefix) = &formula.prefix {
            reference_roots.push(prefix.clone());
        }
        reference_roots.extend(formula.relevant_files.iter().map(PathBuf::from));
    }
    reference_roots.sort();
    reference_roots.dedup();
    let reference_scan = detect_references(
        context,
        &reference_roots,
        remove_nvm,
        remove_fnm,
        remove_pnpm,
    );
    let global_failures = globals
        .results
        .iter()
        .filter(|result| result.status != GlobalStatus::Installed)
        .cloned()
        .collect();
    let global_fingerprint = global_inventory_fingerprint(&fresh_globals);
    let mut runtime_fingerprint = environment
        .runtimes
        .iter()
        .map(|runtime| {
            format!(
                "{:?}\0{}\0{}\0{}\0{}\0{}",
                runtime.manager,
                runtime.provider,
                runtime.version,
                runtime.root.display(),
                runtime.node_path.display(),
                runtime
                    .npm_path
                    .as_deref()
                    .map(Path::display)
                    .map(|path| path.to_string())
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    runtime_fingerprint.sort();
    let mut diagnostics = globals.inventory.diagnostics.clone();
    diagnostics.extend(environment.diagnostics);
    diagnostics.extend(fresh_globals.diagnostics.clone());
    diagnostics.extend(shell_diagnostics);
    if global_fingerprint != global_inventory_fingerprint(&globals.inventory) {
        diagnostics.push(
            "legacy global inventory changed during migration; refusing old-env cleanup".to_owned(),
        );
    }
    diagnostics.sort();
    diagnostics.dedup();
    AggressiveCleanupPlan {
        targets,
        shell_changes,
        nvm_roots,
        fnm_data_roots,
        fnm_multishell_roots,
        cargo_fnm,
        references: reference_scan.facts,
        unaffected_reference_count: reference_scan.unaffected_count,
        global_failures,
        unreconstructable_globals: fresh_globals.unreconstructable,
        global_fingerprint,
        runtime_fingerprint,
        diagnostics,
    }
}

fn global_inventory_fingerprint(inventory: &GlobalInventory) -> Vec<String> {
    let mut fingerprint = inventory
        .packages
        .iter()
        .map(|package| {
            format!(
                "{}\0{}\0{}\0{}\0{:?}",
                package.provider,
                package.node_version.as_deref().unwrap_or_default(),
                package.name,
                package.version,
                package.source
            )
        })
        .collect::<Vec<_>>();
    fingerprint.sort();
    fingerprint
}

pub fn execute_cleanup(context: &AppContext<'_>, plan: &AggressiveCleanupPlan) -> StageOutcome {
    let mut outcome = cleanup_shell_configuration(context, &plan.shell_changes);
    if outcome.incomplete {
        outcome.note(
            "shell cleanup 未完整完成；已跳过 Homebrew/pnpm/nvm/fnm/Cargo provider cleanup"
                .to_owned(),
        );
        return outcome;
    }
    merge_outcome(&mut outcome, execute_targets(context, &plan.targets));
    for root in &plan.nvm_roots {
        match remove_nvm_root(&context.home.home, root) {
            Ok(()) => outcome.note(format!("已删除 nvm data {}", root.display())),
            Err(error) => {
                outcome.failure(format!("删除 nvm data {} 失败：{error}", root.display()))
            }
        }
    }
    for root in &plan.fnm_data_roots {
        match remove_fnm_data_root(&context.home.home, root) {
            Ok(()) => outcome.note(format!("已删除 fnm data {}", root.display())),
            Err(error) => {
                outcome.failure(format!("删除 fnm data {} 失败：{error}", root.display()))
            }
        }
    }
    for root in &plan.fnm_multishell_roots {
        match remove_fnm_multishell_root(&context.home.home, root) {
            Ok(()) => outcome.note(format!("已删除 fnm multishell {}", root.display())),
            Err(error) => outcome.failure(format!(
                "删除 fnm multishell {} 失败：{error}",
                root.display()
            )),
        }
    }
    if plan.cargo_fnm {
        match run_cargo_uninstall_fnm(context) {
            Ok(()) => outcome.note("已执行 cargo uninstall fnm".to_owned()),
            Err(error) => outcome.failure(format!("cargo uninstall fnm 失败：{error}")),
        }
    }
    if !plan.references.is_empty() {
        outcome.note(format!(
            "已报告 {} 条旧 runtime 风险引用",
            plan.references.len()
        ));
    }
    if plan.unaffected_reference_count > 0 {
        outcome.note(format!(
            "已过滤 {} 条不受 cleanup 影响的引用候选",
            plan.unaffected_reference_count
        ));
    }
    for failed in &plan.global_failures {
        outcome.failure(format!(
            "Vite+ global 未达到目标：{}@{}",
            failed.name, failed.expected_version
        ));
    }
    outcome
}

pub fn decode_vp_globals(value: &str) -> Result<BTreeMap<String, String>> {
    let value = serde_json::from_str::<Value>(value).map_err(|error| AppError::Decode {
        action: "decode vp list -g --json".to_owned(),
        detail: error.to_string(),
    })?;
    let values = value
        .as_array()
        .or_else(|| value.get("packages").and_then(Value::as_array))
        .ok_or_else(|| AppError::Decode {
            action: "decode vp list -g --json".to_owned(),
            detail: "expected array or packages array".to_owned(),
        })?;
    let mut packages = BTreeMap::new();
    for package in values {
        let Some(name) = package.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(version) = package.get("version").and_then(Value::as_str) else {
            continue;
        };
        packages.insert(name.to_owned(), version.to_owned());
    }
    Ok(packages)
}

fn usable_vp(
    context: &AppContext<'_>,
    vp: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Option<String> {
    vp.is_file()
        .then(|| run_program(context, vp, environment, None, ["--version"]))
        .transpose()
        .ok()
        .flatten()
        .and_then(|result| result.success().then_some(result))
        .and_then(|result| first_version(&format!("{}\n{}", result.stdout, result.stderr)))
}

fn install_vp(
    context: &AppContext<'_>,
    home: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<String> {
    let installer = Builder::new()
        .prefix("jt-vite-plus-")
        .tempfile_in(&context.home.temp_root)
        .map_err(|error| AppError::io("create Vite+ installer file", None, error))?;
    let installer_path = installer.path().to_path_buf();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&installer_path, fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                AppError::io(
                    "set installer permissions",
                    Some(installer_path.clone()),
                    error,
                )
            },
        )?;
    }
    let curl = first_executable("curl", &context.environment)
        .ok_or_else(|| AppError::Invalid("curl is required for Vite+ installation".to_owned()))?;
    let mut download = CommandSpec::new(
        curl,
        [
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
            "--retry-all-errors",
            "--connect-timeout",
            "10",
            "--max-time",
            "300",
            "--output",
            installer_path.to_string_lossy().as_ref(),
            VITE_PLUS_INSTALLER_URL,
        ],
    );
    download.env = environment.clone();
    download.clear_env = true;
    context
        .runner
        .run(&download)?
        .require_success("download Vite+ installer", &[])?;
    let mut execute = CommandSpec::new("bash", [installer_path.to_string_lossy().as_ref()]);
    execute.env = environment.clone();
    execute.clear_env = true;
    execute.remove_env.extend([
        OsString::from("VP_VERSION"),
        OsString::from("VP_LOCAL_TGZ"),
        OsString::from("VP_PR_VERSION"),
    ]);
    context
        .runner
        .run(&execute)?
        .require_success("run Vite+ upstream installer", &[])?;
    usable_vp(context, &home.join("bin/vp"), environment).ok_or_else(|| {
        AppError::Invalid("upstream installer produced no usable canonical Vite+ CLI".to_owned())
    })
}

fn prewarm_pnpm(
    context: &AppContext<'_>,
    pnpm: &Path,
    environment: &BTreeMap<OsString, OsString>,
    version: &str,
) -> Result<()> {
    let fixture = TempDir::new_in(&context.home.temp_root)
        .map_err(|error| AppError::io("create pnpm fixture", None, error))?;
    let package = serde_json::json!({
        "name": format!("jt-pnpm-warm-{version}"),
        "private": true,
        "packageManager": format!("pnpm@{version}"),
    });
    fs::write(
        fixture.path().join("package.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&package).expect("serializable fixture")
        ),
    )
    .map_err(|error| AppError::io("write pnpm fixture", None, error))?;
    require_expected_version(
        run_program(
            context,
            pnpm,
            environment,
            Some(fixture.path()),
            ["--version"],
        )?,
        version,
        &format!("prewarm pnpm {version}"),
    )?;
    Ok(())
}

fn install_node14_x64(
    context: &AppContext<'_>,
    vp: &Path,
    home: &Path,
    environment: &BTreeMap<OsString, OsString>,
    version: &str,
    workspace: &Path,
) -> Result<()> {
    let curl = first_executable("curl", &context.environment).ok_or_else(|| {
        AppError::Invalid("curl is required for macOS x64 Vite+ bootstrap".to_owned())
    })?;
    let archive = workspace.join("vite-plus-cli-darwin-x64.tgz");
    let unpacked = workspace.join("vite-plus-x64");
    fs::create_dir_all(&unpacked).map_err(|error| {
        AppError::io("create x64 Vite+ directory", Some(unpacked.clone()), error)
    })?;
    let url = format!(
        "{PACKAGE_REGISTRY}@voidzero-dev/vite-plus-cli-darwin-x64/-/vite-plus-cli-darwin-x64-{version}.tgz"
    );
    let mut download = CommandSpec::new(
        curl,
        [
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
            "--output",
            archive.to_string_lossy().as_ref(),
            url.as_str(),
        ],
    );
    download.env = environment.clone();
    download.clear_env = true;
    context
        .runner
        .run(&download)?
        .require_success("download matching x64 Vite+ CLI", &[])?;
    let mut unpack = CommandSpec::new(
        "tar",
        [
            "-xzf",
            archive.to_string_lossy().as_ref(),
            "--strip-components=1",
            "-C",
            unpacked.to_string_lossy().as_ref(),
        ],
    );
    unpack.env = environment.clone();
    unpack.clear_env = true;
    context
        .runner
        .run(&unpack)?
        .require_success("unpack matching x64 Vite+ CLI", &[])?;
    let x64_vp = unpacked.join("vp");
    let mut inspect = CommandSpec::new(
        "arch",
        ["-x86_64", x64_vp.to_string_lossy().as_ref(), "--version"],
    );
    inspect.env = environment.clone();
    inspect.clear_env = true;
    let actual = require_version(
        context.runner.run(&inspect)?,
        "verify matching x64 Vite+ CLI",
    )?;
    if actual != version {
        return Err(AppError::Invalid(format!(
            "matching x64 Vite+ CLI version differs: expected {version}, got {actual}"
        )));
    }
    let mut command = CommandSpec::new(
        "arch",
        [
            "-x86_64",
            x64_vp.to_string_lossy().as_ref(),
            "env",
            "install",
            "14.21.3",
        ],
    )
    .cwd(workspace);
    command.env = environment.clone();
    command.clear_env = true;
    command
        .env
        .insert(OsString::from("VP_HOME"), OsString::from(home.as_os_str()));
    context
        .runner
        .run(&command)?
        .require_success("preinstall Node 14 x64 through matching Vite+ CLI", &[])?;
    let _ = vp;
    Ok(())
}

fn configure_shell_blocks(context: &AppContext<'_>, snapshots: &mut [FileSnapshot]) -> Result<()> {
    for (shell, path, enabled) in shell_config_plan(&context.home.home, &context.environment)
        .into_iter()
        .filter(|(_, path, enabled)| path.exists() || *enabled)
    {
        let snapshot = snapshots
            .iter_mut()
            .find(|snapshot| snapshot.path == path)
            .ok_or_else(|| {
                AppError::Invalid(format!("missing shell snapshot: {}", path.display()))
            })?;
        let current = read_optional(&path)?;
        let text = match current.as_deref() {
            Some(content) => std::str::from_utf8(content).map_err(|error| AppError::Decode {
                action: format!("decode shell config {}", path.display()),
                detail: error.to_string(),
            })?,
            None => "",
        };
        let next = reconcile_vite_loader(text, shell, enabled);
        if current.as_deref() == Some(next.as_bytes()) || current.is_none() && next.is_empty() {
            continue;
        }
        let expected_written = next.into_bytes();
        atomic_write(
            &context.home.home,
            &path,
            current.as_deref(),
            &expected_written,
        )?;
        snapshot.expected_written = Some(expected_written);
    }
    Ok(())
}

#[derive(Clone)]
struct FileSnapshot {
    path: PathBuf,
    original: Option<Vec<u8>>,
    expected_written: Option<Vec<u8>>,
}

fn configuration_snapshots(context: &AppContext<'_>) -> Result<Vec<FileSnapshot>> {
    shell_config_plan(&context.home.home, &context.environment)
        .into_iter()
        .map(|(_, path, _)| path)
        .map(|path| {
            let original = read_optional(&path)?;
            Ok(FileSnapshot {
                path,
                original,
                expected_written: None,
            })
        })
        .collect()
}

fn restore_configuration_snapshots(
    context: &AppContext<'_>,
    snapshots: &[FileSnapshot],
) -> Result<()> {
    for snapshot in snapshots {
        let Some(expected) = &snapshot.expected_written else {
            continue;
        };
        let current = read_optional(&snapshot.path)?;
        if current.as_deref() != Some(expected.as_slice()) {
            continue;
        }
        match (&snapshot.original, current.as_deref()) {
            (None, Some(_)) => remove_file_safe(&context.home.home, &snapshot.path)?,
            (Some(original), _) => {
                atomic_write(&context.home.home, &snapshot.path, Some(expected), original)?
            }
            (None, None) => {}
        }
    }
    Ok(())
}

fn vite_environment(context: &AppContext<'_>, home: &Path) -> BTreeMap<OsString, OsString> {
    let mut environment = inherited_proxy_env(&context.environment);
    environment.insert(
        OsString::from("HOME"),
        context.home.home.as_os_str().to_os_string(),
    );
    environment.insert(OsString::from("VP_HOME"), home.as_os_str().to_os_string());
    environment.insert(
        OsString::from("VP_NODE_DIST_MIRROR"),
        OsString::from(NODE_MIRROR),
    );
    environment.insert(
        OsString::from("NPM_CONFIG_REGISTRY"),
        OsString::from(PACKAGE_REGISTRY),
    );
    environment.insert(
        OsString::from("npm_config_registry"),
        OsString::from(PACKAGE_REGISTRY),
    );
    let inherited_path = context.env("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(home.join("bin"))
            .chain(std::env::split_paths(&OsString::from(inherited_path))),
    )
    .unwrap_or_default();
    environment.insert(OsString::from("PATH"), path);
    environment.insert(OsString::from("CI"), OsString::from("true"));
    environment
}

fn run_vp<const N: usize>(
    context: &AppContext<'_>,
    vp: &Path,
    environment: &BTreeMap<OsString, OsString>,
    cwd: &Path,
    args: [&str; N],
    action: &str,
) -> Result<CommandResult> {
    let result = run_program(context, vp, environment, Some(cwd), args)?;
    result.require_success(action, &[])?;
    Ok(result)
}

fn run_vp_vec(
    context: &AppContext<'_>,
    vp: &Path,
    environment: &BTreeMap<OsString, OsString>,
    cwd: &Path,
    args: Vec<String>,
) -> Result<CommandResult> {
    let mut command = CommandSpec::new(vp, args);
    command.cwd = Some(cwd.to_path_buf());
    command.env = environment.clone();
    command.clear_env = true;
    context.runner.run(&command)
}

fn run_program<const N: usize>(
    context: &AppContext<'_>,
    program: &Path,
    environment: &BTreeMap<OsString, OsString>,
    cwd: Option<&Path>,
    args: [&str; N],
) -> Result<CommandResult> {
    let mut command = CommandSpec::new(program, args);
    command.env = environment.clone();
    command.clear_env = true;
    command.cwd = cwd.map(Path::to_path_buf);
    context.runner.run(&command)
}

fn require_version(result: CommandResult, action: &str) -> Result<String> {
    result.require_success(action, &[])?;
    first_version(&format!("{}\n{}", result.stdout, result.stderr)).ok_or_else(|| {
        AppError::Decode {
            action: action.to_owned(),
            detail: "no exact semver in output".to_owned(),
        }
    })
}

fn require_expected_version(result: CommandResult, expected: &str, action: &str) -> Result<()> {
    let actual = require_version(result, action)?;
    if actual != expected {
        return Err(AppError::Invalid(format!(
            "{action}: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn first_version(value: &str) -> Option<String> {
    value
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '.' || character == '-')
        })
        .find_map(|token| {
            let token = token.trim_start_matches('v');
            semver::Version::parse(token)
                .ok()
                .filter(|parsed| parsed.to_string() == token)
                .map(|_| token.to_owned())
        })
}

fn global_results(
    candidates: &[GlobalCandidate],
    actual: &BTreeMap<String, String>,
    bulk_error: Option<&str>,
) -> Vec<GlobalResult> {
    candidates
        .iter()
        .map(|candidate| {
            let actual_version = actual.get(&candidate.name);
            let status = if actual_version == Some(&candidate.version) {
                GlobalStatus::Installed
            } else {
                GlobalStatus::Failed
            };
            let detail = match (status, actual_version, bulk_error) {
                (GlobalStatus::Installed, _, _) => None,
                (_, Some(actual), _) => {
                    Some(format!("expected {}, got {actual}", candidate.version))
                }
                (_, None, Some(error)) => Some(error.to_owned()),
                (_, None, None) => Some("missing from vp list -g --json".to_owned()),
            };
            GlobalResult {
                name: candidate.name.clone(),
                expected_version: candidate.version.clone(),
                status,
                detail,
            }
        })
        .collect()
}

fn command_detail(result: &CommandResult) -> String {
    let detail = result
        .stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .or_else(|| {
            result
                .stdout
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
        })
        .unwrap_or("vp install -g returned non-zero")
        .trim()
        .to_owned();
    redact(&detail, &[])
}

fn fnm_multishell_roots(context: &AppContext<'_>) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    for candidate in [
        context.env("FNM_MULTISHELL_PATH").map(PathBuf::from),
        Some(context.home.home.join(".local/state/fnm_multishells")),
        Some(context.home.home.join("Library/Caches/fnm_multishells")),
        context
            .env("XDG_RUNTIME_DIR")
            .map(|root| PathBuf::from(root).join("fnm_multishells")),
        context
            .env("XDG_STATE_HOME")
            .map(|root| PathBuf::from(root).join("fnm_multishells")),
    ] {
        if let Some(root) = candidate
            .filter(|path| path.is_dir())
            .and_then(|path| fnm_multishell_root(&path))
        {
            roots.insert(root);
        }
    }
    roots.into_iter().collect()
}

fn safe_nvm_root(home: &Path, root: &Path) -> bool {
    root == home.join(".nvm") && ensure_safe_home_target(home, root).is_ok()
}

fn safe_fnm_root(home: &Path, root: &Path) -> bool {
    [
        home.join(".fnm"),
        home.join(".local/share/fnm"),
        home.join("Library/Application Support/fnm"),
    ]
    .contains(&root.to_path_buf())
        && ensure_safe_home_target(home, root).is_ok()
}

fn safe_fnm_multishell_root(home: &Path, root: &Path) -> bool {
    [
        home.join(".local/state/fnm_multishells"),
        home.join("Library/Caches/fnm_multishells"),
    ]
    .contains(&root.to_path_buf())
        && ensure_safe_home_target(home, root).is_ok()
}

fn fnm_multishell_root(candidate: &Path) -> Option<PathBuf> {
    candidate
        .ancestors()
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name == "fnm_multishells")
        })
        .map(Path::to_path_buf)
}

fn fnm_launchers(context: &AppContext<'_>) -> Vec<PathBuf> {
    let mut launchers = BTreeSet::new();
    for candidate in [
        context.home.home.join(".fnm/bin/fnm"),
        context.home.home.join(".local/share/fnm/fnm"),
        context.home.home.join(".local/bin/fnm"),
        context.home.home.join(".cargo/bin/fnm"),
    ] {
        if candidate.is_file() || candidate.is_symlink() {
            launchers.insert(candidate);
        }
    }
    for candidate in executable_candidates("fnm", &context.environment) {
        if candidate.starts_with(&context.home.home) {
            launchers.insert(candidate);
        }
    }
    launchers.into_iter().collect()
}

fn cargo_has_fnm(context: &AppContext<'_>) -> bool {
    let Some(cargo) = first_executable("cargo", &context.environment) else {
        return false;
    };
    context
        .runner
        .run(&CommandSpec::new(cargo, ["install", "--list"]))
        .map(|result| {
            result.success()
                && result
                    .stdout
                    .lines()
                    .any(|line| line.trim_start().starts_with("fnm v"))
        })
        .unwrap_or(false)
}

fn run_cargo_uninstall_fnm(context: &AppContext<'_>) -> Result<()> {
    let cargo = first_executable("cargo", &context.environment)
        .ok_or_else(|| AppError::Invalid("cargo disappeared during fnm cleanup".to_owned()))?;
    let result = context
        .runner
        .run(&CommandSpec::new(cargo, ["uninstall", "fnm"]))?;
    result.require_success("cargo uninstall fnm", &[])
}

fn deduplicate_cleanup_targets(targets: &mut Vec<CleanupTarget>) {
    let mut seen = BTreeSet::new();
    targets.retain(|target| seen.insert(format!("{}\0{}", target.label, target.action)));
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::BTreeMap,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    use tempfile::tempdir;

    use crate::node::{
        cli::Prompter,
        command::{CommandResult, CommandSpec, Runner},
        context::AppContext,
        error::{AppError, Result},
        inventory::GlobalInventory,
        model::{
            CleanupAction, GlobalCandidate, GlobalPackage, GlobalStatus, NODE_MIRROR,
            PACKAGE_REGISTRY, PackageSource,
        },
        platform::HomePaths,
    };

    use super::{
        AggressiveCleanupPlan, FileSnapshot, GlobalStage, VitePlusInstall, cleanup_plan,
        configuration_snapshots, configure_global_environment, configure_shell_blocks,
        decode_vp_globals, execute_cleanup, fnm_multishell_root, global_inventory_fingerprint,
        global_results, install_vp, progress_step, require_expected_version,
        restore_configuration_snapshots, verify_vp_global_candidates, vite_environment,
    };

    #[test]
    fn normalizes_fnm_session_path_to_multishell_root() {
        let root = Path::new("/home/me/.local/state/fnm_multishells");
        let session = root.join("12345/bin");

        assert_eq!(fnm_multishell_root(&session).as_deref(), Some(root));
        assert_eq!(fnm_multishell_root(Path::new("/home/me/.fnm")), None);
    }

    #[test]
    fn progress_step_reports_success_and_failure() {
        let root = tempdir().unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = ProgressPrompt::default();
        {
            let context = AppContext {
                runner: &runner,
                prompt: &mut prompt,
                home: HomePaths {
                    home: root.path().join("home"),
                    temp_root: root.path().to_path_buf(),
                },
                environment: BTreeMap::new(),
            };

            assert_eq!(
                progress_step(&context, 4, "预装 Node", || Ok(42)).unwrap(),
                42
            );
            assert!(
                progress_step(&context, 5, "预装 Node", || -> Result<()> {
                    Err(AppError::Invalid("failed".to_owned()))
                })
                .is_err()
            );
        }

        assert_eq!(
            prompt.events.into_inner(),
            [
                "start:[4/12] 预装 Node",
                "finish:[4/12] 预装 Node：完成",
                "start:[5/12] 预装 Node",
                "fail:[5/12] 预装 Node：失败",
            ]
        );
    }

    #[test]
    fn installer_file_survives_download_and_execution() {
        let root = tempdir().unwrap();
        let home = root.path().join("home/.vite-plus");
        let path_bin = root.path().join("bin");
        fs::create_dir_all(home.join("bin")).unwrap();
        fs::create_dir_all(&path_bin).unwrap();
        fs::write(path_bin.join("curl"), "").unwrap();
        fs::write(home.join("bin/vp"), "").unwrap();
        let environment =
            BTreeMap::from([(OsString::from("PATH"), path_bin.as_os_str().to_os_string())]);
        let runner = InstallerRunner;
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: root.path().join("home"),
                temp_root: root.path().to_path_buf(),
            },
            environment: environment.clone(),
        };

        assert_eq!(install_vp(&context, &home, &environment).unwrap(), "1.2.3");
    }

    #[test]
    fn vp_json_decoder_ignores_extra_fields() {
        let packages =
            decode_vp_globals(r#"[{"name":"eslint","version":"9.10.0","path":"/ignored"}]"#)
                .unwrap();

        assert_eq!(packages.get("eslint").map(String::as_str), Some("9.10.0"));
    }

    #[test]
    fn pnpm_prewarm_rejects_wrong_resolved_version() {
        let result = CommandResult {
            status: 0,
            stdout: "9.7.0\n".to_owned(),
            stderr: String::new(),
        };

        assert!(require_expected_version(result, "10.12.4", "prewarm").is_err());
    }

    #[test]
    fn global_inventory_fingerprint_detects_migration_drift() {
        let mut initial = GlobalInventory {
            packages: vec![GlobalPackage {
                name: "eslint".to_owned(),
                version: "9.10.0".to_owned(),
                source: PackageSource::Registry,
                provider: "nvm 20".to_owned(),
                node_version: Some("20.11.0".to_owned()),
                bins: vec!["eslint".to_owned()],
            }],
            ..GlobalInventory::default()
        };
        let baseline = global_inventory_fingerprint(&initial);

        initial.packages[0].version = "9.11.0".to_owned();

        assert_ne!(baseline, global_inventory_fingerprint(&initial));
    }

    #[test]
    fn pnpm_home_with_unproven_globals_is_report_only() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        let pnpm_home = home.join("Library/pnpm");
        let package = pnpm_home.join("global/5/node_modules/tool/package.json");
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        fs::write(&package, r#"{"name":"tool","version":"1.0.0"}"#).unwrap();
        fs::write(pnpm_home.join("pnpm"), "").unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home,
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([
                (
                    OsString::from("PNPM_HOME"),
                    pnpm_home.as_os_str().to_os_string(),
                ),
                (OsString::from("PATH"), pnpm_home.as_os_str().to_os_string()),
            ]),
        };
        let globals = GlobalStage {
            inventory: GlobalInventory::default(),
            results: Vec::new(),
        };

        let plan = cleanup_plan(&context, &globals);
        let target = plan
            .targets
            .iter()
            .find(|target| target.label.starts_with("PNPM_HOME"))
            .unwrap();

        assert_eq!(target.action, CleanupAction::ReportOnly);
        assert!(target.evidence.contains("provenance"));
    }

    #[test]
    fn broad_fnm_root_and_unowned_launcher_are_report_only() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        let broad_root = home.join(".local/share");
        let bin = home.join("bin");
        fs::create_dir_all(broad_root.join("node-versions")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("fnm"), "#!/bin/sh\n").unwrap();
        fs::write(
            home.join(".zshrc"),
            format!("export FNM_DIR=\"{}\"\n", broad_root.display()),
        )
        .unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home,
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([
                (
                    OsString::from("FNM_DIR"),
                    broad_root.as_os_str().to_os_string(),
                ),
                (OsString::from("PATH"), bin.as_os_str().to_os_string()),
            ]),
        };
        let globals = GlobalStage {
            inventory: GlobalInventory::default(),
            results: Vec::new(),
        };

        let plan = cleanup_plan(&context, &globals);

        assert!(plan.fnm_data_roots.is_empty());
        assert!(plan.shell_changes.is_empty());
        assert!(
            plan.targets
                .iter()
                .any(|target| target.label.starts_with("custom fnm root")
                    && target.action == CleanupAction::ReportOnly)
        );
        assert!(
            plan.targets
                .iter()
                .any(|target| target.label.starts_with("unverified fnm launcher")
                    && target.action == CleanupAction::ReportOnly)
        );
    }

    #[test]
    fn global_readback_marks_bulk_partial_results_without_retry() {
        let candidates = vec![
            candidate("eslint", "9.10.0"),
            candidate("typescript", "5.6.3"),
        ];
        let actual = BTreeMap::from([("eslint".to_owned(), "9.10.0".to_owned())]);
        let results = global_results(&candidates, &actual, Some("network failed"));

        assert_eq!(results[0].status, GlobalStatus::Installed);
        assert_eq!(results[1].status, GlobalStatus::Failed);
    }

    #[test]
    fn fresh_vp_readback_rejects_changed_candidate() {
        let root = tempdir().unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: root.path().join("home"),
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([(OsString::from("PATH"), OsString::from("/usr/bin"))]),
        };
        let install = VitePlusInstall {
            vp: PathBuf::from("/vp"),
            home: root.path().join("home/.vite-plus"),
            version: "1.2.3".to_owned(),
            default_node: "24.15.0".to_owned(),
            default_pnpm: "10.12.4".to_owned(),
        };

        assert!(
            verify_vp_global_candidates(&context, &install, &[candidate("eslint", "9.10.0")])
                .is_ok()
        );
        assert!(
            verify_vp_global_candidates(&context, &install, &[candidate("eslint", "9.11.0")])
                .is_err()
        );
    }

    #[test]
    fn vite_calls_get_taobao_mirror_and_only_standard_proxy_environment() {
        let root = tempdir().unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: root.path().join("home"),
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([
                (OsString::from("PATH"), OsString::from("/usr/bin")),
                (
                    OsString::from("HTTPS_PROXY"),
                    OsString::from("http://127.0.0.1:7890"),
                ),
                (OsString::from("UNRELATED"), OsString::from("ignored")),
            ]),
        };

        let environment = vite_environment(&context, Path::new("/work/.vite-plus"));

        assert_eq!(
            environment.get(&OsString::from("NPM_CONFIG_REGISTRY")),
            Some(&OsString::from(PACKAGE_REGISTRY))
        );
        assert_eq!(
            environment.get(&OsString::from("VP_NODE_DIST_MIRROR")),
            Some(&OsString::from(NODE_MIRROR))
        );
        assert!(environment.contains_key(&OsString::from("HTTPS_PROXY")));
        assert!(!environment.contains_key(&OsString::from("UNRELATED")));
        assert!(
            environment
                .get(&OsString::from("PATH"))
                .is_some_and(|path| path.to_string_lossy().starts_with("/work/.vite-plus/bin"))
        );
    }

    #[test]
    fn configuration_restore_only_replaces_the_expected_managed_shell_content() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir(&home).unwrap();
        let config = home.join(".zshrc");
        fs::write(&config, "original\n").unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: home.clone(),
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::new(),
        };
        let snapshot = FileSnapshot {
            path: config.clone(),
            original: Some(b"original\n".to_vec()),
            expected_written: Some(b"managed\n".to_vec()),
        };

        fs::write(&config, "user changed\n").unwrap();
        restore_configuration_snapshots(&context, std::slice::from_ref(&snapshot)).unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), "user changed\n");

        fs::write(&config, "managed\n").unwrap();
        restore_configuration_snapshots(&context, &[snapshot]).unwrap();
        assert_eq!(fs::read_to_string(config).unwrap(), "original\n");
    }

    #[test]
    fn shell_configuration_refuses_invalid_utf8() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir(&home).unwrap();
        let config = home.join(".zshrc");
        let original = [0xff, b'\n'];
        fs::write(&config, original).unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: home.clone(),
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::new(),
        };
        let mut snapshots = configuration_snapshots(&context).unwrap();

        assert!(configure_shell_blocks(&context, &mut snapshots).is_err());
        assert_eq!(fs::read(config).unwrap(), original);
    }

    #[test]
    fn shell_cleanup_failure_keeps_legacy_runtime() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        let nvm = home.join(".nvm");
        fs::create_dir_all(nvm.join("versions/node")).unwrap();
        fs::write(nvm.join("nvm.sh"), "").unwrap();
        let config = home.join(".zshrc");
        fs::write(&config, "concurrent user edit\n").unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home,
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::new(),
        };
        let plan = AggressiveCleanupPlan {
            shell_changes: vec![crate::node::cleanup::ShellCleanup {
                path: config,
                expected: b"previewed\n".to_vec(),
                content: b"managed\n".to_vec(),
            }],
            nvm_roots: vec![nvm.clone()],
            ..AggressiveCleanupPlan::default()
        };

        let outcome = execute_cleanup(&context, &plan);

        assert!(outcome.incomplete);
        assert!(nvm.is_dir());
    }

    #[test]
    fn fish_configuration_creates_zshenv_and_converges_duplicate_loaders() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        let bin = root.path().join("bin");
        fs::create_dir_all(home.join(".config/fish/conf.d")).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("zsh"), "").unwrap();
        fs::write(bin.join("fish"), "").unwrap();
        fs::write(
            home.join(".zshrc"),
            "keep-zsh\n# Vite+ bin (https://viteplus.dev)\n. \"$HOME/.vite-plus/env\"\n",
        )
        .unwrap();
        fs::write(
            home.join(".config/fish/config.fish"),
            "keep-fish\n# >>> nlab-node-env-init vite-plus >>>\nold\n# <<< nlab-node-env-init vite-plus <<<\n",
        )
        .unwrap();
        fs::write(
            home.join(".config/fish/conf.d/vite-plus.fish"),
            "# Vite+ bin (https://viteplus.dev)\nsource \"$HOME/.vite-plus/env.fish\"\n",
        )
        .unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: home.clone(),
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([
                (
                    OsString::from("SHELL"),
                    OsString::from("/opt/homebrew/bin/fish"),
                ),
                (OsString::from("PATH"), bin.as_os_str().to_os_string()),
            ]),
        };
        let mut snapshots = configuration_snapshots(&context).unwrap();

        configure_shell_blocks(&context, &mut snapshots).unwrap();

        let zshenv = fs::read_to_string(home.join(".zshenv")).unwrap();
        let zshrc = fs::read_to_string(home.join(".zshrc")).unwrap();
        let fish_config = fs::read_to_string(home.join(".config/fish/config.fish")).unwrap();
        let fish_snippet =
            fs::read_to_string(home.join(".config/fish/conf.d/vite-plus.fish")).unwrap();
        assert!(zshenv.contains("jt node init vite-plus"));
        assert!(zshenv.contains("$VP_HOME/env"));
        assert_eq!(zshrc, "keep-zsh\n");
        assert_eq!(fish_config, "keep-fish\n");
        assert!(fish_snippet.contains("jt node init vite-plus"));
        assert_eq!(fish_snippet.matches("env.fish").count(), 2);
    }

    #[test]
    fn bulk_failure_still_reads_once_and_uses_actual_vp_global_list() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        let runtime = home.join(".nvm/versions/node/v20.11.0/bin");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(home.join(".nvm/nvm.sh"), "").unwrap();
        fs::write(runtime.join("node"), "").unwrap();
        fs::write(runtime.join("npm"), "").unwrap();
        let runner = GlobalRunner::default();
        let mut prompt = NoopPrompt;
        let mut environment = BTreeMap::new();
        environment.insert(OsString::from("PATH"), OsString::from("/usr/bin"));
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: home.clone(),
                temp_root: root.path().to_path_buf(),
            },
            environment,
        };
        let install = VitePlusInstall {
            vp: PathBuf::from("/nlab/vp"),
            home: home.join(".vite-plus"),
            version: "1.2.3".to_owned(),
            default_node: "22.0.0".to_owned(),
            default_pnpm: "10.0.0".to_owned(),
        };

        let stage = configure_global_environment(&context, &install).unwrap();

        assert_eq!(stage.results[0].status, GlobalStatus::Installed);
        let commands = runner.commands.lock().unwrap();
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.args.first().is_some_and(|arg| arg == "install"))
                .count(),
            1
        );
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.args.first().is_some_and(|arg| arg == "list"))
                .count(),
            1
        );
    }

    fn candidate(name: &str, version: &str) -> GlobalCandidate {
        GlobalCandidate {
            name: name.to_owned(),
            version: version.to_owned(),
            origins: vec![GlobalPackage {
                name: name.to_owned(),
                version: version.to_owned(),
                source: PackageSource::Registry,
                provider: "test".to_owned(),
                node_version: None,
                bins: Vec::new(),
            }],
        }
    }

    #[derive(Default)]
    struct GlobalRunner {
        commands: Mutex<Vec<CommandSpec>>,
    }

    impl Runner for GlobalRunner {
        fn run(&self, command: &CommandSpec) -> Result<CommandResult> {
            self.commands.lock().unwrap().push(command.clone());
            let program = command
                .program
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let first = command
                .args
                .first()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if program == "npm" && first == "ls" {
                return Ok(CommandResult {
                    status: 0,
                    stdout: r#"{"dependencies":{"eslint":{"version":"9.10.0","resolved":"https://registry.npmjs.org/eslint/-/eslint-9.10.0.tgz"}}}"#.to_owned(),
                    stderr: String::new(),
                });
            }
            if first == "install" {
                return Ok(CommandResult {
                    status: 1,
                    stdout: String::new(),
                    stderr: "simulated partial failure".to_owned(),
                });
            }
            if first == "list" {
                return Ok(CommandResult {
                    status: 0,
                    stdout: r#"[{"name":"eslint","version":"9.10.0","extra":true}]"#.to_owned(),
                    stderr: String::new(),
                });
            }
            Ok(CommandResult {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    struct NoopPrompt;

    #[derive(Default)]
    struct ProgressPrompt {
        events: RefCell<Vec<String>>,
    }

    impl Prompter for ProgressPrompt {
        fn confirm(&mut self, _: &str) -> Result<bool> {
            unreachable!()
        }

        fn start_progress(&self, message: &str) -> Result<()> {
            self.events.borrow_mut().push(format!("start:{message}"));
            Ok(())
        }

        fn finish_progress(&self, message: &str) -> Result<()> {
            self.events.borrow_mut().push(format!("finish:{message}"));
            Ok(())
        }

        fn fail_progress(&self, message: &str) -> Result<()> {
            self.events.borrow_mut().push(format!("fail:{message}"));
            Ok(())
        }
    }

    struct InstallerRunner;

    impl Runner for InstallerRunner {
        fn run(&self, command: &CommandSpec) -> Result<CommandResult> {
            match command
                .program
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
            {
                "curl" => {
                    assert!(command.args.iter().any(|arg| arg == "--retry-all-errors"));
                    assert!(
                        command
                            .args
                            .windows(2)
                            .any(|args| args[0] == "--retry" && args[1] == "3")
                    );
                    let output = command
                        .args
                        .iter()
                        .position(|arg| arg == "--output")
                        .map(|index| PathBuf::from(&command.args[index + 1]))
                        .unwrap();
                    assert!(output.is_file());
                }
                "bash" => assert!(PathBuf::from(&command.args[0]).is_file()),
                "vp" => {
                    return Ok(CommandResult {
                        status: 0,
                        stdout: "vp 1.2.3".to_owned(),
                        stderr: String::new(),
                    });
                }
                program => panic!("unexpected command: {program}"),
            }
            Ok(CommandResult {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    impl Prompter for NoopPrompt {
        fn confirm(&mut self, _: &str) -> Result<bool> {
            Ok(true)
        }
    }
}
