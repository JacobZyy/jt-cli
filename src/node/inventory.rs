use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use semver::Version;
use serde_json::Value;

use crate::node::{
    command::{CommandResult, CommandSpec},
    context::AppContext,
    model::{
        FormulaFact, GlobalCandidate, GlobalPackage, ManagerFact, ManagerKind, PackageSource,
        PnpmProvider, PnpmProviderKind, ReferenceFact, ReferenceImpact, RuntimeFact,
    },
    platform::{executable_candidates, first_executable, is_regular_or_symlink_file},
    shell::shell_config_plan,
};

#[derive(Clone, Debug, Default)]
pub struct EnvironmentInventory {
    pub managers: Vec<ManagerFact>,
    pub runtimes: Vec<RuntimeFact>,
    pub formulas: Vec<FormulaFact>,
    pub pnpm_home: Vec<PathBuf>,
    pub pnpm_providers: Vec<PnpmProvider>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GlobalInventory {
    pub packages: Vec<GlobalPackage>,
    pub candidates: Vec<GlobalCandidate>,
    pub unreconstructable: Vec<GlobalPackage>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ReferenceScan {
    pub facts: Vec<ReferenceFact>,
    pub unaffected_count: usize,
}

struct ReferenceScope<'a> {
    home: &'a Path,
    impact_roots: &'a [PathBuf],
    remove_nvm: bool,
    remove_fnm: bool,
    remove_pnpm: bool,
}

pub fn inspect_environment(context: &AppContext<'_>) -> EnvironmentInventory {
    let managers = detect_managers(context);
    let mut diagnostics = Vec::new();
    let formulas = detect_homebrew_formulas(context, &mut diagnostics);
    let runtimes = discover_runtimes(context, &managers, &formulas, &mut diagnostics);
    let pnpm_home = discover_pnpm_homes(context);
    let pnpm_providers =
        discover_pnpm_providers(context, &runtimes, &formulas, &pnpm_home, &mut diagnostics);
    EnvironmentInventory {
        managers,
        runtimes,
        formulas,
        pnpm_home,
        pnpm_providers,
        diagnostics,
    }
}

pub fn detect_managers(context: &AppContext<'_>) -> Vec<ManagerFact> {
    let nvm_root = context
        .env("NVM_DIR")
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| context.home.home.join(".nvm"));
    let mut managers = Vec::new();
    if nvm_root.join("nvm.sh").is_file() {
        managers.push(ManagerFact {
            kind: ManagerKind::Nvm,
            executable: None,
            root: nvm_root,
        });
    }

    let fnm_executable = first_executable("fnm", &context.environment);
    let fnm_roots = fnm_root_candidates(context);
    let reported_root = fnm_executable
        .as_ref()
        .and_then(|executable| fnm_root_from_command(context, executable));
    if let Some(root) = reported_root
        .filter(|root| root.join("node-versions").is_dir())
        .or_else(|| {
            fnm_roots
                .iter()
                .find(|root| root.join("node-versions").is_dir())
                .cloned()
        })
        .or_else(|| {
            fnm_executable
                .as_ref()
                .and_then(|_| fnm_roots.first().cloned())
        })
    {
        managers.push(ManagerFact {
            kind: ManagerKind::Fnm,
            executable: fnm_executable,
            root,
        });
    }
    managers
}

pub fn discover_runtimes(
    context: &AppContext<'_>,
    managers: &[ManagerFact],
    formulas: &[FormulaFact],
    diagnostics: &mut Vec<String>,
) -> Vec<RuntimeFact> {
    let mut runtimes = Vec::new();
    for manager in managers {
        match manager.kind {
            ManagerKind::Nvm => {
                let root = manager.root.join("versions/node");
                for (version, path) in version_directories(&root, diagnostics) {
                    push_runtime(
                        &mut runtimes,
                        RuntimeFact {
                            manager: Some(ManagerKind::Nvm),
                            provider: "nvm".to_owned(),
                            version,
                            node_path: path.join("bin/node"),
                            npm_path: optional_file(path.join("bin/npm")),
                            root: path,
                        },
                        diagnostics,
                    );
                }
            }
            ManagerKind::Fnm => {
                let mut roots = BTreeSet::new();
                roots.insert(manager.root.clone());
                roots.extend(fnm_root_candidates(context));
                for fnm_root in roots {
                    let root = fnm_root.join("node-versions");
                    for (version, version_root) in version_directories(&root, diagnostics) {
                        let path = version_root.join("installation");
                        push_runtime(
                            &mut runtimes,
                            RuntimeFact {
                                manager: Some(ManagerKind::Fnm),
                                provider: "fnm".to_owned(),
                                version,
                                node_path: path.join("bin/node"),
                                npm_path: optional_file(path.join("bin/npm")),
                                root: path,
                            },
                            diagnostics,
                        );
                    }
                }
            }
        }
    }

    for formula in formulas {
        if !is_node_formula(&formula.name) {
            continue;
        }
        let Some(prefix) = &formula.prefix else {
            continue;
        };
        let node = prefix.join("bin/node");
        if !is_regular_or_symlink_file(&node) {
            continue;
        }
        let version = command_version(context, &node).or_else(|| formula.version.clone());
        let Some(version) = version.and_then(|version| exact_version(&version)) else {
            continue;
        };
        push_runtime(
            &mut runtimes,
            RuntimeFact {
                manager: None,
                provider: format!("homebrew/{}", formula.name),
                version,
                root: prefix.clone(),
                node_path: node,
                npm_path: optional_file(prefix.join("bin/npm")),
            },
            diagnostics,
        );
    }
    runtimes.sort_by(|left, right| {
        left.version
            .cmp(&right.version)
            .then_with(|| left.provider.cmp(&right.provider))
    });
    runtimes
}

pub fn discover_pnpm_homes(context: &AppContext<'_>) -> Vec<PathBuf> {
    let mut values = BTreeSet::new();
    if let Some(value) = context.env("PNPM_HOME")
        && !value.trim().is_empty()
    {
        values.insert(PathBuf::from(value));
    }
    for candidate in [
        context.home.home.join("Library/pnpm"),
        context.home.home.join(".local/share/pnpm"),
        context.home.home.join(".pnpm"),
    ] {
        if candidate.exists() {
            values.insert(candidate);
        }
    }
    for (_, path, _) in shell_config_plan(&context.home.home, &context.environment) {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            if let Some(candidate) = pnpm_home_from_shell_line(line, &context.home.home) {
                values.insert(candidate);
            }
        }
    }
    values.into_iter().collect()
}

pub fn discover_pnpm_providers(
    context: &AppContext<'_>,
    runtimes: &[RuntimeFact],
    formulas: &[FormulaFact],
    pnpm_homes: &[PathBuf],
    diagnostics: &mut Vec<String>,
) -> Vec<PnpmProvider> {
    let mut providers = Vec::new();
    let mut known_paths = BTreeSet::new();
    let pnpm_globals = read_pnpm_home_globals(pnpm_homes, diagnostics);

    for runtime in runtimes {
        let bin = runtime.root.join("bin");
        let pnpm = bin.join("pnpm");
        let pnpx = optional_file(bin.join("pnpx"));
        let npm_packages = npm_globals(context, runtime, diagnostics);
        let corepack = bin.join("corepack");
        if is_corepack_owned(&runtime.root, &corepack, &pnpm, pnpx.as_deref()) {
            known_paths.insert(path_key(&pnpm));
            providers.push(PnpmProvider {
                kind: PnpmProviderKind::Corepack,
                pnpm_path: pnpm.clone(),
                pnpx_path: pnpx.clone(),
                real_path: pnpm.canonicalize().ok(),
                version: command_version(context, &pnpm),
                node_version: Some(runtime.version.clone()),
                prefix: None,
                globals: Vec::new(),
                detail: format!("{} target Corepack shim", runtime.provider),
            });
        }
        if npm_packages
            .iter()
            .any(|package| package.name == "pnpm" || package.name == "@pnpm/exe")
        {
            let prefix = npm_global_root(context, runtime);
            let package = npm_packages
                .iter()
                .find(|package| package.name == "pnpm" || package.name == "@pnpm/exe")
                .map(|package| package.name.clone())
                .unwrap_or_else(|| "pnpm".to_owned());
            known_paths.insert(path_key(&pnpm));
            providers.push(PnpmProvider {
                kind: PnpmProviderKind::NpmGlobal,
                pnpm_path: pnpm.clone(),
                pnpx_path: pnpx,
                real_path: pnpm.canonicalize().ok(),
                version: command_version(context, &pnpm),
                node_version: Some(runtime.version.clone()),
                prefix,
                globals: vec![package],
                detail: format!("{} npm global", runtime.provider),
            });
        }
    }

    for formula in formulas.iter().filter(|formula| {
        formula.name == "pnpm"
            || formula
                .relevant_files
                .iter()
                .any(|file| file.ends_with("/bin/pnpm"))
    }) {
        let Some(prefix) = &formula.prefix else {
            continue;
        };
        let pnpm = prefix.join("bin/pnpm");
        if !is_regular_or_symlink_file(&pnpm) {
            continue;
        }
        known_paths.insert(path_key(&pnpm));
        providers.push(PnpmProvider {
            kind: PnpmProviderKind::Homebrew,
            pnpm_path: pnpm.clone(),
            pnpx_path: optional_file(prefix.join("bin/pnpx")),
            real_path: pnpm.canonicalize().ok(),
            version: command_version(context, &pnpm),
            node_version: None,
            prefix: Some(prefix.clone()),
            globals: pnpm_globals.clone(),
            detail: "Homebrew formula pnpm".to_owned(),
        });
    }

    for pnpm in executable_candidates("pnpm", &context.environment) {
        if known_paths.contains(&path_key(&pnpm)) {
            continue;
        }
        let parent = pnpm.parent().map(Path::to_path_buf);
        let pnpx = parent
            .as_ref()
            .and_then(|parent| optional_file(parent.join("pnpx")));
        let standalone_home = pnpm_homes
            .iter()
            .find(|home| pnpm.starts_with(home))
            .cloned();
        let (kind, detail) = if standalone_home.is_some() {
            (
                PnpmProviderKind::Standalone,
                "PNPM_HOME launcher".to_owned(),
            )
        } else {
            (
                PnpmProviderKind::Unknown,
                "PATH candidate has no verified owner".to_owned(),
            )
        };
        providers.push(PnpmProvider {
            kind,
            pnpm_path: pnpm.clone(),
            pnpx_path: pnpx,
            real_path: pnpm.canonicalize().ok(),
            version: command_version(context, &pnpm),
            node_version: None,
            prefix: standalone_home,
            globals: if matches!(kind, PnpmProviderKind::Standalone) {
                pnpm_globals.clone()
            } else {
                Vec::new()
            },
            detail,
        });
    }

    let mut known_executables = providers
        .iter()
        .flat_map(|provider| std::iter::once(&provider.pnpm_path).chain(provider.pnpx_path.iter()))
        .map(|path| path_key(path))
        .collect::<BTreeSet<_>>();
    for pnpx in executable_candidates("pnpx", &context.environment) {
        if !known_executables.insert(path_key(&pnpx)) {
            continue;
        }
        let standalone_home = pnpm_homes
            .iter()
            .find(|home| pnpx.starts_with(home))
            .cloned();
        let (kind, detail) = if standalone_home.is_some() {
            (
                PnpmProviderKind::Standalone,
                "PNPM_HOME pnpx launcher without paired pnpm candidate".to_owned(),
            )
        } else {
            (
                PnpmProviderKind::Unknown,
                "PATH pnpx candidate has no verified owner".to_owned(),
            )
        };
        providers.push(PnpmProvider {
            kind,
            pnpm_path: pnpx.clone(),
            pnpx_path: Some(pnpx.clone()),
            real_path: pnpx.canonicalize().ok(),
            version: command_version(context, &pnpx),
            node_version: None,
            prefix: standalone_home,
            globals: if matches!(kind, PnpmProviderKind::Standalone) {
                pnpm_globals.clone()
            } else {
                Vec::new()
            },
            detail,
        });
    }
    providers.sort_by(|left, right| left.pnpm_path.cmp(&right.pnpm_path));
    providers
}

pub fn inventory_legacy_globals(
    context: &AppContext<'_>,
    environment: &EnvironmentInventory,
) -> GlobalInventory {
    let mut diagnostics = environment.diagnostics.clone();
    let mut packages = environment
        .runtimes
        .iter()
        .flat_map(|runtime| npm_globals(context, runtime, &mut diagnostics))
        .collect::<Vec<_>>();
    packages.extend(read_pnpm_global_packages(
        &environment.pnpm_home,
        &mut diagnostics,
    ));
    packages.extend(read_bun_global_packages(context, &mut diagnostics));
    let (candidates, unreconstructable) = build_global_candidates(&packages);
    GlobalInventory {
        packages,
        candidates,
        unreconstructable,
        diagnostics,
    }
}

pub fn build_global_candidates(
    packages: &[GlobalPackage],
) -> (Vec<GlobalCandidate>, Vec<GlobalPackage>) {
    let excluded = ["npm", "corepack", "pnpm", "nrm"];
    let mut grouped = BTreeMap::<String, Vec<GlobalPackage>>::new();
    let mut unreconstructable = Vec::new();
    for package in packages {
        if excluded.contains(&package.name.as_str()) {
            continue;
        }
        if !valid_package_name(&package.name)
            || exact_semver(&package.version).is_none()
            || !matches!(package.source, PackageSource::Registry)
        {
            unreconstructable.push(package.clone());
            continue;
        }
        grouped
            .entry(package.name.clone())
            .or_default()
            .push(package.clone());
    }
    let mut candidates = Vec::new();
    for (name, records) in grouped {
        let selected = records
            .iter()
            .max_by_key(|record| exact_semver(&record.version).expect("validated semver"))
            .expect("non-empty group");
        candidates.push(GlobalCandidate {
            name,
            version: selected.version.clone(),
            origins: records,
        });
    }
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    unreconstructable.sort_by(|left, right| left.name.cmp(&right.name));
    (candidates, unreconstructable)
}

pub fn detect_references(
    context: &AppContext<'_>,
    impact_roots: &[PathBuf],
    remove_nvm: bool,
    remove_fnm: bool,
    remove_pnpm: bool,
) -> ReferenceScan {
    let mut result = ReferenceScan::default();
    let scope = ReferenceScope {
        home: &context.home.home,
        impact_roots,
        remove_nvm,
        remove_fnm,
        remove_pnpm,
    };
    let explicit = [
        context.home.home.join(".zshrc"),
        context.home.home.join(".zprofile"),
        context.home.home.join(".bashrc"),
        context.home.home.join(".bash_profile"),
        context.home.home.join(".profile"),
        context.home.home.join(".config/fish/config.fish"),
        context.home.home.join(".config/fish/conf.d/fnm.fish"),
    ];
    for path in explicit {
        scan_reference_file(&path, &scope, &mut result);
    }
    for root in [
        context.home.home.join("Library/LaunchAgents"),
        PathBuf::from("/Library/LaunchAgents"),
        PathBuf::from("/Library/LaunchDaemons"),
        context.home.home.join(".config"),
        context.home.home.join(".local/bin"),
        context.home.home.join("bin"),
    ] {
        scan_reference_tree(&root, 3, &scope, &mut result);
    }
    scan_reference_file(
        &context.home.home.join(".pm2/dump.pm2"),
        &scope,
        &mut result,
    );
    if let Some(crontab) = optional_command(context, CommandSpec::new("crontab", ["-l"]))
        && crontab.success()
    {
        scan_reference_text("crontab", &crontab.stdout, &scope, &mut result);
    }
    if let Some(processes) =
        optional_command(context, CommandSpec::new("ps", ["-axo", "pid=,command="]))
        && processes.success()
    {
        scan_reference_text("process", &processes.stdout, &scope, &mut result);
    }
    result
        .facts
        .sort_by(|left, right| left.location.cmp(&right.location));
    result
        .facts
        .dedup_by(|left, right| left.location == right.location && left.excerpt == right.excerpt);
    result
}

fn fnm_root_candidates(context: &AppContext<'_>) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    if let Some(root) = context.env("FNM_DIR")
        && !root.trim().is_empty()
    {
        roots.insert(PathBuf::from(root));
    }
    roots.insert(context.home.home.join(".fnm"));
    roots.insert(context.home.home.join(".local/share/fnm"));
    roots.insert(context.home.home.join("Library/Application Support/fnm"));
    roots.into_iter().collect()
}

fn pnpm_home_from_shell_line(line: &str, home: &Path) -> Option<PathBuf> {
    if !line.contains("PNPM_HOME") {
        return None;
    }
    if let Some(relative) = line.find("$HOME/") {
        let value = line[relative + "$HOME/".len()..]
            .split(['\'', '"', ';', ' ', '\t'])
            .next()
            .unwrap_or_default();
        return (!value.is_empty()).then(|| home.join(value));
    }
    let value = line
        .split_once("PNPM_HOME")?
        .1
        .trim_start_matches([' ', '\t', '='])
        .trim_start_matches([' ', '\t', '\'', '"'])
        .split(['\'', '"', ';', ' ', '\t'])
        .next()
        .unwrap_or_default();
    Path::new(value).is_absolute().then(|| PathBuf::from(value))
}

fn fnm_root_from_command(context: &AppContext<'_>, executable: &Path) -> Option<PathBuf> {
    let result = optional_command(context, CommandSpec::new(executable, ["env", "--json"]))?;
    if !result.success() {
        return None;
    }
    let value = serde_json::from_str::<Value>(&result.stdout).ok()?;
    find_named_string(&value, "FNM_DIR").map(PathBuf::from)
}

fn find_named_string(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(values) => values
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                values
                    .values()
                    .find_map(|value| find_named_string(value, key))
            }),
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_named_string(value, key)),
        _ => None,
    }
}

fn version_directories(root: &Path, diagnostics: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            diagnostics.push(format!(
                "cannot read runtime directory {}: {error}",
                root.display()
            ));
            return Vec::new();
        }
    };
    entries
        .filter_map(|entry| match entry {
            Err(error) => {
                diagnostics.push(format!(
                    "cannot read runtime entry in {}: {error}",
                    root.display()
                ));
                None
            }
            Ok(entry) => {
                let path = entry.path();
                let is_directory = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata.file_type().is_dir(),
                    Err(error) => {
                        diagnostics.push(format!(
                            "cannot inspect runtime entry {}: {error}",
                            path.display()
                        ));
                        return None;
                    }
                };
                if !is_directory {
                    return None;
                }
                match exact_version(&entry.file_name().to_string_lossy()) {
                    Some(version) => Some((version, path)),
                    None => {
                        diagnostics.push(format!("unknown runtime directory: {}", path.display()));
                        None
                    }
                }
            }
        })
        .collect()
}

fn exact_version(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('v');
    (Version::parse(value).ok()?.to_string() == value).then(|| value.to_owned())
}

fn optional_file(path: PathBuf) -> Option<PathBuf> {
    is_regular_or_symlink_file(&path).then_some(path)
}

fn push_runtime(
    runtimes: &mut Vec<RuntimeFact>,
    runtime: RuntimeFact,
    diagnostics: &mut Vec<String>,
) {
    if !is_regular_or_symlink_file(&runtime.node_path) {
        diagnostics.push(format!(
            "runtime lacks executable node: {}",
            runtime.node_path.display()
        ));
        return;
    }
    if runtime.npm_path.is_none() {
        diagnostics.push(format!(
            "runtime lacks executable npm: {}",
            runtime.root.display()
        ));
    }
    if runtimes
        .iter()
        .any(|known| path_key(&known.node_path) == path_key(&runtime.node_path))
    {
        return;
    }
    runtimes.push(runtime);
}

fn is_node_formula(name: &str) -> bool {
    name == "node" || name.starts_with("node@")
}

fn is_relevant_homebrew_file(file: &str) -> bool {
    [
        "/bin/node",
        "/bin/npm",
        "/bin/npx",
        "/bin/corepack",
        "/bin/pnpm",
        "/bin/pnpx",
        "/bin/fnm",
        "/bin/nvm",
        "/bin/nrm",
    ]
    .iter()
    .any(|suffix| file.ends_with(suffix))
}

fn command_version(context: &AppContext<'_>, program: &Path) -> Option<String> {
    optional_command(context, CommandSpec::new(program, ["--version"]))
        .filter(CommandResult::success)
        .and_then(|result| first_exact_version(&format!("{}\n{}", result.stdout, result.stderr)))
}

fn first_exact_version(value: &str) -> Option<String> {
    value
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '.' || character == '-')
        })
        .find_map(exact_version)
}

fn optional_command(context: &AppContext<'_>, command: CommandSpec) -> Option<CommandResult> {
    context.runner.run(&command).ok()
}

fn detect_homebrew_formulas(
    context: &AppContext<'_>,
    diagnostics: &mut Vec<String>,
) -> Vec<FormulaFact> {
    let Some(brew) = first_executable("brew", &context.environment) else {
        return Vec::new();
    };
    let listed = match context.runner.run(&CommandSpec::new(
        &brew,
        ["list", "--formula", "--versions"],
    )) {
        Ok(result) if result.success() => result,
        Ok(_) => {
            diagnostics.push("Homebrew formula inventory failed".to_owned());
            return Vec::new();
        }
        Err(error) => {
            diagnostics.push(format!("cannot run Homebrew formula inventory: {error}"));
            return Vec::new();
        }
    };
    let mut formulas = Vec::new();
    for line in listed.stdout.lines() {
        let mut tokens = line.split_whitespace();
        let Some(name) = tokens.next() else { continue };
        let version = tokens.next().map(str::to_owned);
        let explicitly_relevant =
            matches!(name, "node" | "pnpm" | "fnm" | "nvm") || name.starts_with("node@");
        let relevant_files = match context
            .runner
            .run(&CommandSpec::new(&brew, ["list", "--verbose", name]))
        {
            Ok(result) if result.success() => result
                .stdout
                .lines()
                .map(str::trim)
                .filter(|file| is_relevant_homebrew_file(file))
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            Ok(_) | Err(_) => {
                diagnostics.push(format!("cannot inspect Homebrew formula files for {name}"));
                Vec::new()
            }
        };
        if !explicitly_relevant && relevant_files.is_empty() {
            continue;
        }
        let prefix = match context
            .runner
            .run(&CommandSpec::new(&brew, ["--prefix", name]))
        {
            Ok(result) if result.success() => {
                let prefix = result
                    .stdout
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from);
                if prefix.is_none() {
                    diagnostics.push(format!("Homebrew prefix is empty for {name}"));
                }
                prefix
            }
            Ok(_) | Err(_) => {
                diagnostics.push(format!("cannot resolve Homebrew prefix for {name}"));
                None
            }
        };
        let installed_dependents = match context
            .runner
            .run(&CommandSpec::new(&brew, ["uses", "--installed", name]))
        {
            Ok(result) if result.success() => result
                .stdout
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
            Ok(_) | Err(_) => {
                diagnostics.push(format!(
                    "cannot inspect installed Homebrew dependents for {name}"
                ));
                Vec::new()
            }
        };
        formulas.push(FormulaFact {
            name: name.to_owned(),
            version,
            prefix,
            installed_dependents,
            relevant_files,
        });
    }
    formulas.sort_by(|left, right| left.name.cmp(&right.name));
    formulas
}

fn is_corepack_owned(
    runtime_root: &Path,
    corepack: &Path,
    pnpm: &Path,
    pnpx: Option<&Path>,
) -> bool {
    if !is_regular_or_symlink_file(corepack)
        || !is_regular_or_symlink_file(pnpm)
        || !pnpx.is_some_and(is_regular_or_symlink_file)
    {
        return false;
    }
    let Some(pnpm_real) = pnpm.canonicalize().ok() else {
        return false;
    };
    let Some(pnpx_real) = pnpx.and_then(|path| path.canonicalize().ok()) else {
        return false;
    };
    let runtime_root = runtime_root
        .canonicalize()
        .unwrap_or_else(|_| runtime_root.to_path_buf());
    let contains_corepack = |path: &Path| {
        path.starts_with(&runtime_root)
            && path
                .components()
                .any(|component| component.as_os_str().to_string_lossy().contains("corepack"))
    };
    contains_corepack(&pnpm_real) && contains_corepack(&pnpx_real)
}

fn npm_globals(
    context: &AppContext<'_>,
    runtime: &RuntimeFact,
    diagnostics: &mut Vec<String>,
) -> Vec<GlobalPackage> {
    let Some(npm) = &runtime.npm_path else {
        return Vec::new();
    };
    let Some(result) = optional_command(
        context,
        CommandSpec::new(npm, ["ls", "-g", "--depth=0", "--json", "--long"]),
    ) else {
        diagnostics.push(format!(
            "cannot run npm global inventory for {}",
            runtime.provider
        ));
        return Vec::new();
    };
    if !result.success() {
        diagnostics.push(format!(
            "npm global inventory failed for {}",
            runtime.provider
        ));
        if result.stdout.trim().is_empty() {
            return Vec::new();
        }
    }
    let Ok(root) = serde_json::from_str::<Value>(&result.stdout) else {
        diagnostics.push(format!("invalid npm global JSON from {}", runtime.provider));
        return Vec::new();
    };
    let Some(dependencies) = root.get("dependencies").and_then(Value::as_object) else {
        diagnostics.push(format!(
            "npm global inventory lacks dependencies from {}",
            runtime.provider
        ));
        return Vec::new();
    };
    let package_root = npm_global_root(context, runtime);
    let mut packages = Vec::new();
    for (name, raw) in dependencies {
        let Some(version) = raw.get("version").and_then(Value::as_str) else {
            diagnostics.push(format!(
                "npm global package {name} lacks version from {}",
                runtime.provider
            ));
            continue;
        };
        let source = package_source(raw);
        let bins = package_root
            .as_ref()
            .map(|root| read_bins(&root.join(name).join("package.json")))
            .unwrap_or_default();
        packages.push(GlobalPackage {
            name: name.to_owned(),
            version: version.to_owned(),
            source,
            provider: runtime.provider.clone(),
            node_version: Some(runtime.version.clone()),
            bins,
        });
    }
    packages
}

fn npm_global_root(context: &AppContext<'_>, runtime: &RuntimeFact) -> Option<PathBuf> {
    let npm = runtime.npm_path.as_ref()?;
    optional_command(context, CommandSpec::new(npm, ["root", "-g"]))
        .filter(CommandResult::success)
        .and_then(|result| {
            result
                .stdout
                .lines()
                .next()
                .map(str::trim)
                .filter(|path| Path::new(path).is_absolute())
                .map(PathBuf::from)
        })
}

fn package_source(value: &Value) -> PackageSource {
    let resolved = value
        .get("resolved")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if value.get("link").and_then(Value::as_bool) == Some(true) || resolved.starts_with("link:") {
        PackageSource::Link
    } else if resolved.starts_with("file:") {
        PackageSource::File
    } else if resolved.starts_with("workspace:") {
        PackageSource::Workspace
    } else if resolved.starts_with("git+")
        || resolved.starts_with("git:")
        || resolved.starts_with("github:")
    {
        PackageSource::Git
    } else if [
        "https://registry.npmjs.org/",
        "http://registry.npmjs.org/",
        "https://registry.npmmirror.com/",
    ]
    .iter()
    .any(|registry| resolved.starts_with(registry))
    {
        PackageSource::Registry
    } else {
        PackageSource::Unknown
    }
}

fn read_pnpm_home_globals(pnpm_homes: &[PathBuf], diagnostics: &mut Vec<String>) -> Vec<String> {
    read_pnpm_global_packages(pnpm_homes, diagnostics)
        .into_iter()
        .map(|package| package.name)
        .collect()
}

fn read_pnpm_global_packages(
    pnpm_homes: &[PathBuf],
    diagnostics: &mut Vec<String>,
) -> Vec<GlobalPackage> {
    let mut packages = Vec::new();
    let mut roots = BTreeSet::new();
    for home in pnpm_homes {
        for base in [
            home.join("global"),
            home.parent()
                .map(|parent| parent.join("global"))
                .unwrap_or_default(),
        ] {
            let entries = match fs::read_dir(&base) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    diagnostics.push(format!(
                        "cannot read pnpm global directory {}: {error}",
                        base.display()
                    ));
                    continue;
                }
            };
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let root = entry.path().join("node_modules");
                        if root.is_dir() {
                            roots.insert(root);
                        } else {
                            diagnostics.push(format!(
                                "pnpm global layout lacks node_modules: {}",
                                entry.path().display()
                            ));
                        }
                    }
                    Err(error) => diagnostics.push(format!(
                        "cannot read pnpm global entry in {}: {error}",
                        base.display()
                    )),
                }
            }
        }
    }
    for root in roots {
        packages.extend(scan_node_modules(&root, "pnpm-home", None, diagnostics));
    }
    packages
}

fn read_bun_global_packages(
    context: &AppContext<'_>,
    diagnostics: &mut Vec<String>,
) -> Vec<GlobalPackage> {
    let global_dir = match context.env("BUN_INSTALL_GLOBAL_DIR") {
        Some(value) if Path::new(&value).is_absolute() => PathBuf::from(value),
        Some(value) => {
            diagnostics.push(format!(
                "relative BUN_INSTALL_GLOBAL_DIR cannot be inventoried safely: {value}"
            ));
            return Vec::new();
        }
        None => context.home.home.join(".bun/install/global"),
    };
    scan_node_modules(&global_dir.join("node_modules"), "bun", None, diagnostics)
}

fn scan_node_modules(
    root: &Path,
    provider: &str,
    node_version: Option<String>,
    diagnostics: &mut Vec<String>,
) -> Vec<GlobalPackage> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            diagnostics.push(format!(
                "cannot read {provider} global directory {}: {error}",
                root.display()
            ));
            return Vec::new();
        }
    };
    let mut package_dirs = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.push(format!(
                    "cannot read {provider} global entry in {}: {error}",
                    root.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if name.starts_with('@') {
            match fs::read_dir(&path) {
                Ok(scoped) => {
                    for entry in scoped {
                        match entry {
                            Ok(entry) => package_dirs.push(entry.path()),
                            Err(error) => diagnostics.push(format!(
                                "cannot read scoped {provider} package in {}: {error}",
                                path.display()
                            )),
                        }
                    }
                }
                Err(error) => diagnostics.push(format!(
                    "cannot read scoped {provider} directory {}: {error}",
                    path.display()
                )),
            }
        } else {
            package_dirs.push(path);
        }
    }
    package_dirs
        .into_iter()
        .filter_map(|path| {
            let package_file = path.join("package.json");
            let text = match fs::read_to_string(&package_file) {
                Ok(text) => text,
                Err(error) => {
                    diagnostics.push(format!(
                        "cannot read {provider} package metadata {}: {error}",
                        package_file.display()
                    ));
                    return None;
                }
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                diagnostics.push(format!(
                    "invalid package metadata: {}",
                    package_file.display()
                ));
                return None;
            };
            let Some(name) = value.get("name").and_then(Value::as_str) else {
                diagnostics.push(format!(
                    "missing package name in {}",
                    package_file.display()
                ));
                return None;
            };
            let Some(version) = value.get("version").and_then(Value::as_str) else {
                diagnostics.push(format!(
                    "missing package version in {}",
                    package_file.display()
                ));
                return None;
            };
            let source = fs::symlink_metadata(&path)
                .ok()
                .filter(|metadata| metadata.file_type().is_symlink())
                .map_or(PackageSource::Unknown, |_| PackageSource::Link);
            Some(GlobalPackage {
                name: name.to_owned(),
                version: version.to_owned(),
                source,
                provider: provider.to_owned(),
                node_version: node_version.clone(),
                bins: bins_from_value(&value),
            })
        })
        .collect()
}

fn read_bins(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|value| bins_from_value(&value))
        .unwrap_or_default()
}

fn bins_from_value(value: &Value) -> Vec<String> {
    match value.get("bin") {
        Some(Value::String(_)) => value
            .get("name")
            .and_then(Value::as_str)
            .map(|name| vec![name.rsplit('/').next().unwrap_or(name).to_owned()])
            .unwrap_or_default(),
        Some(Value::Object(values)) => values.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn valid_package_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 214 {
        return false;
    }
    let valid_part = |part: &str| {
        !part.is_empty()
            && !part.starts_with('-')
            && part.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
            })
    };
    match value.strip_prefix('@') {
        Some(scoped) => scoped.split_once('/').is_some_and(|(scope, name)| {
            !name.contains('/') && valid_part(scope) && valid_part(name)
        }),
        None => !value.contains('/') && valid_part(value),
    }
}

fn exact_semver(value: &str) -> Option<Version> {
    Version::parse(value)
        .ok()
        .filter(|parsed| parsed.to_string() == value)
}

fn path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn scan_reference_tree(
    root: &Path,
    depth: usize,
    scope: &ReferenceScope<'_>,
    result: &mut ReferenceScan,
) {
    if depth == 0 || root.file_name().is_some_and(|name| name == ".proto") {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if should_skip_reference_path(&path) {
                continue;
            }
            scan_reference_tree(&path, depth - 1, scope, result);
        } else if metadata.is_file() {
            scan_reference_file(&path, scope, result);
        }
    }
}

fn scan_reference_file(path: &Path, scope: &ReferenceScope<'_>, result: &mut ReferenceScan) {
    if should_skip_reference_path(path) {
        return;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() > 1_000_000 {
        return;
    }
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    scan_reference_text(&path.display().to_string(), &content, scope, result);
}

fn scan_reference_text(
    source: &str,
    content: &str,
    scope: &ReferenceScope<'_>,
    result: &mut ReferenceScan,
) {
    for (index, line) in content.lines().enumerate() {
        if !is_reference_candidate(line) {
            continue;
        }
        match classify_reference(line, scope, source) {
            Some((impact, evidence)) => result.facts.push(ReferenceFact {
                source: source.to_owned(),
                location: format!("{source}:{}", index + 1),
                excerpt: line.trim().chars().take(240).collect(),
                impact,
                evidence,
            }),
            None => result.unaffected_count += 1,
        }
    }
}

fn is_reference_candidate(line: &str) -> bool {
    let markers = [
        ".nvm",
        ".fnm",
        "fnm_multishells",
        "FNM_MULTISHELL_PATH",
        "nvm use",
        "nvm exec",
        "fnm env",
        "fnm use",
        "fnm exec",
    ];
    markers.iter().any(|marker| line.contains(marker))
        || (line.contains('/')
            && ["/node", "/npm", "/npx", "/pnpm", "/pnpx"]
                .iter()
                .any(|suffix| line.contains(suffix)))
}

fn classify_reference(
    line: &str,
    scope: &ReferenceScope<'_>,
    source: &str,
) -> Option<(ReferenceImpact, String)> {
    let trimmed = line.trim();
    if trimmed.starts_with('#')
        || (source == "process"
            && (trimmed.contains("nlab-node-env-init") || trimmed.contains("jt node init")))
    {
        return None;
    }
    let home = scope.home.display().to_string();
    let expanded = trimmed
        .replace("${HOME}", &home)
        .replace("$HOME", &home)
        .replace('~', &home);
    if let Some(root) = scope.impact_roots.iter().find(|root| {
        let root = root.display().to_string();
        !root.is_empty() && expanded.contains(&root)
    }) {
        return Some((
            ReferenceImpact::Affected,
            format!("命中将清理路径 {}", root.display()),
        ));
    }
    let direct_nvm = ["nvm use", "nvm exec", "nvm.sh"]
        .iter()
        .any(|marker| trimmed.contains(marker));
    if scope.remove_nvm && direct_nvm {
        return Some((ReferenceImpact::Affected, "调用将移除的 nvm".to_owned()));
    }
    let direct_fnm = ["fnm env", "fnm use", "fnm exec"]
        .iter()
        .any(|marker| trimmed.contains(marker));
    if scope.remove_fnm && direct_fnm {
        return Some((ReferenceImpact::Affected, "调用将移除的 fnm".to_owned()));
    }
    let uncertain_nvm = scope.remove_nvm && ["NVM_DIR", ".nvm"].iter().any(|m| trimmed.contains(m));
    let uncertain_fnm = scope.remove_fnm
        && ["FNM_DIR", ".fnm", "fnm_multishells", "FNM_MULTISHELL_PATH"]
            .iter()
            .any(|marker| trimmed.contains(marker));
    let uncertain_pnpm = scope.remove_pnpm && trimmed.contains("PNPM_HOME");
    if uncertain_nvm || uncertain_fnm || uncertain_pnpm {
        return Some((
            ReferenceImpact::Uncertain,
            "含动态旧环境变量，无法静态解析 exact path".to_owned(),
        ));
    }
    None
}

fn should_skip_reference_path(path: &Path) -> bool {
    if path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("node_modules" | "store" | ".cache" | "Caches")
        )
    }) {
        return true;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name == "package-lock.json"
        || name == "pnpm-lock.yaml"
        || name == "bun.lock"
        || name == "bun.lockb"
        || name.ends_with(".lock")
        || name.contains(".bak")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString, fs, path::PathBuf};

    use crate::node::{
        cli::Prompter,
        command::{CommandResult, CommandSpec, Runner},
        context::AppContext,
        error::Result,
        model::{GlobalPackage, PackageSource, ReferenceImpact, RuntimeFact},
        platform::HomePaths,
    };

    use super::{
        ReferenceScope, build_global_candidates, classify_reference, inspect_environment,
        is_corepack_owned, npm_globals, package_source, read_bun_global_packages,
        scan_node_modules, should_skip_reference_path, valid_package_name,
    };

    #[test]
    fn reference_classifier_only_marks_cleanup_targets_and_dynamic_manager_calls() {
        let home = PathBuf::from("/home/me");
        let fnm = home.join(".local/share/fnm");
        let roots = vec![fnm.clone(), home.join("Library/pnpm")];
        let scope = ReferenceScope {
            home: &home,
            impact_roots: &roots,
            remove_nvm: false,
            remove_fnm: true,
            remove_pnpm: false,
        };

        let affected = classify_reference(
            "legacy=/home/me/.local/share/fnm/node-versions/v20/bin/node",
            &scope,
            "fixture",
        )
        .unwrap();
        let uncertain =
            classify_reference("export FNM_DIR=\"$HOME/custom-fnm\"", &scope, "fixture").unwrap();

        assert_eq!(affected.0, ReferenceImpact::Affected);
        assert_eq!(uncertain.0, ReferenceImpact::Uncertain);
        assert!(
            classify_reference(
                "https://example.test/node_ai_transpond/send",
                &scope,
                "fixture",
            )
            .is_none()
        );
        assert!(
            classify_reference("# stale fnm_multishells sessions", &scope, "fixture",).is_none()
        );
        let pnpm_scope = ReferenceScope {
            remove_pnpm: true,
            ..scope
        };
        assert_eq!(
            classify_reference(
                "export PNPM_HOME=\"$XDG_DATA_HOME/pnpm\"",
                &pnpm_scope,
                "fixture",
            )
            .map(|result| result.0),
            Some(ReferenceImpact::Uncertain)
        );
    }

    #[test]
    fn reference_scan_skips_dependency_metadata_and_backups() {
        assert!(should_skip_reference_path(
            PathBuf::from("package-lock.json").as_path()
        ));
        assert!(should_skip_reference_path(
            PathBuf::from(".config/opencode/node_modules/pkg/package.json").as_path()
        ));
        assert!(should_skip_reference_path(
            PathBuf::from(".config/fish/config.fish.bak.123").as_path()
        ));
        assert!(!should_skip_reference_path(
            PathBuf::from(".config/fish/config.fish").as_path()
        ));
    }

    #[test]
    fn chooses_the_highest_exact_registry_version_and_excludes_tools() {
        let packages = vec![
            package("eslint", "9.1.0", PackageSource::Registry),
            package("eslint", "9.10.0", PackageSource::Registry),
            package("pnpm", "10.0.0", PackageSource::Registry),
            package("local", "1.0.0", PackageSource::File),
        ];
        let (candidates, unresolved) = build_global_candidates(&packages);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "eslint");
        assert_eq!(candidates[0].version, "9.10.0");
        assert_eq!(unresolved[0].name, "local");
    }

    #[test]
    fn unsafe_package_names_are_never_command_arguments() {
        assert!(valid_package_name("eslint"));
        assert!(valid_package_name("@scope/tool"));
        assert!(!valid_package_name("--all"));
        assert!(!valid_package_name("@scope/--all"));
        assert!(!valid_package_name("scope/tool"));
    }

    #[test]
    fn unreadable_global_metadata_records_diagnostic() {
        let root = tempfile::tempdir().unwrap();
        let modules = root.path().join("node_modules");
        fs::create_dir_all(modules.join("broken")).unwrap();
        let mut diagnostics = Vec::new();

        let packages = scan_node_modules(&modules, "fixture", None, &mut diagnostics);

        assert!(packages.is_empty());
        assert!(
            diagnostics
                .iter()
                .any(|message| message.contains("broken/package.json"))
        );
    }

    #[test]
    fn unproven_package_sources_are_not_treated_as_registry() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("node_modules/tool/package.json");
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        fs::write(
            &package,
            r#"{"name":"tool","version":"1.0.0","resolved":"https://evil.test/tool.tgz"}"#,
        )
        .unwrap();
        let mut diagnostics = Vec::new();

        let packages = scan_node_modules(
            &root.path().join("node_modules"),
            "pnpm-home",
            None,
            &mut diagnostics,
        );

        assert_eq!(packages[0].source, PackageSource::Unknown);
        assert_eq!(
            package_source(&serde_json::from_str("{}").unwrap()),
            PackageSource::Unknown
        );
        assert_eq!(
            package_source(
                &serde_json::from_str(r#"{"resolved":"https://evil.test/tool.tgz"}"#).unwrap()
            ),
            PackageSource::Unknown
        );
        assert_eq!(
            package_source(
                &serde_json::from_str(
                    r#"{"resolved":"https://registry.npmjs.org/tool/-/tool-1.0.0.tgz"}"#
                )
                .unwrap()
            ),
            PackageSource::Registry
        );
    }

    #[test]
    fn bun_inventory_honors_explicit_global_directory() {
        let root = tempfile::tempdir().unwrap();
        let global_dir = root.path().join("custom-bun-global");
        let package = global_dir.join("node_modules/tool/package.json");
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        fs::write(&package, r#"{"name":"tool","version":"1.0.0"}"#).unwrap();
        let runner = StaticRunner {
            status: 0,
            stdout: "",
        };
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: root.path().to_path_buf(),
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([(
                OsString::from("BUN_INSTALL_GLOBAL_DIR"),
                global_dir.as_os_str().to_os_string(),
            )]),
        };
        let mut diagnostics = Vec::new();

        let packages = read_bun_global_packages(&context, &mut diagnostics);

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].source, PackageSource::Unknown);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn incomplete_npm_json_records_diagnostic() {
        let root = tempfile::tempdir().unwrap();
        let runtime = RuntimeFact {
            manager: None,
            provider: "fixture".to_owned(),
            version: "20.11.0".to_owned(),
            root: root.path().join("runtime"),
            node_path: root.path().join("runtime/bin/node"),
            npm_path: Some(root.path().join("runtime/bin/npm")),
        };

        for (json, expected) in [
            ("{}", "lacks dependencies"),
            (r#"{"dependencies":{"broken":{}}}"#, "broken lacks version"),
        ] {
            let runner = StaticRunner {
                status: 0,
                stdout: json,
            };
            let mut prompt = NoopPrompt;
            let context = AppContext {
                runner: &runner,
                prompt: &mut prompt,
                home: HomePaths {
                    home: root.path().to_path_buf(),
                    temp_root: root.path().to_path_buf(),
                },
                environment: BTreeMap::new(),
            };
            let mut diagnostics = Vec::new();

            assert!(npm_globals(&context, &runtime, &mut diagnostics).is_empty());
            assert!(diagnostics.iter().any(|message| message.contains(expected)));
        }
    }

    #[test]
    fn failed_homebrew_inventory_records_diagnostic() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("brew"), "").unwrap();
        let runner = StaticRunner {
            status: 1,
            stdout: "",
        };
        let mut prompt = NoopPrompt;
        let context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home: root.path().to_path_buf(),
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([(OsString::from("PATH"), bin.as_os_str().to_os_string())]),
        };

        let inventory = inspect_environment(&context);

        assert!(
            inventory
                .diagnostics
                .iter()
                .any(|message| message.contains("Homebrew formula inventory failed"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn requires_both_pnpm_and_pnpx_to_resolve_to_corepack_inside_runtime() {
        use std::os::unix::fs::symlink;
        use tempfile::tempdir;

        let root = tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let bin = runtime.join("bin");
        let corepack_dir = runtime.join("lib/node_modules/corepack");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&corepack_dir).unwrap();
        fs::write(corepack_dir.join("pnpm.js"), "").unwrap();
        fs::write(corepack_dir.join("pnpx.js"), "").unwrap();
        fs::write(bin.join("corepack"), "").unwrap();
        symlink("../lib/node_modules/corepack/pnpm.js", bin.join("pnpm")).unwrap();
        symlink("../lib/node_modules/corepack/pnpx.js", bin.join("pnpx")).unwrap();

        assert!(is_corepack_owned(
            &runtime,
            &bin.join("corepack"),
            &bin.join("pnpm"),
            Some(&bin.join("pnpx")),
        ));
    }

    fn package(name: &str, version: &str, source: PackageSource) -> GlobalPackage {
        GlobalPackage {
            name: name.to_owned(),
            version: version.to_owned(),
            source,
            provider: "test".to_owned(),
            node_version: None,
            bins: Vec::new(),
        }
    }

    struct StaticRunner {
        status: i32,
        stdout: &'static str,
    }

    impl Runner for StaticRunner {
        fn run(&self, command: &CommandSpec) -> Result<CommandResult> {
            let stdout = if command.args.first().is_some_and(|arg| arg == "root") {
                ""
            } else {
                self.stdout
            };
            Ok(CommandResult {
                status: self.status,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }
    }

    struct NoopPrompt;

    impl Prompter for NoopPrompt {
        fn confirm(&mut self, _: &str) -> Result<bool> {
            Ok(false)
        }
    }
}
