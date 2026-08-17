use std::{
    collections::BTreeSet,
    env, fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Command,
};

use clap::ValueEnum;
use serde_json::{Map, Value};

use crate::node::fs::{atomic_write, read_optional};

const HOOK_DIR: &str = ".codex/hooks/jt-ai-hook";
const PRE_TOOL_USE_FILE: &str = ".codex/hooks/jt-ai-hook/pre-tool-use.ts";
const POST_TOOL_USE_FILE: &str = ".codex/hooks/jt-ai-hook/post-tool-use.ts";
const STOP_ENTRY_FILE: &str = ".codex/hooks/jt-ai-hook/stop-entry.ts";
const OWNED_MARKER: &str = "jt-ai-hook";

const COMMON_FILES: [(&str, &[u8]); 9] = [
    ("files.ts", include_bytes!("../templates/ai-hook/files.ts")),
    (
        "post-tool-use.ts",
        include_bytes!("../templates/ai-hook/post-tool-use.ts"),
    ),
    (
        "pre-tool-use.ts",
        include_bytes!("../templates/ai-hook/pre-tool-use.ts"),
    ),
    (
        "protocol.ts",
        include_bytes!("../templates/ai-hook/protocol.ts"),
    ),
    (
        "runtime.ts",
        include_bytes!("../templates/ai-hook/runtime.ts"),
    ),
    (
        "stop-entry.ts",
        include_bytes!("../templates/ai-hook/stop-entry.ts"),
    ),
    (
        "stop/process.ts",
        include_bytes!("../templates/ai-hook/stop/process.ts"),
    ),
    (
        "stop/types.ts",
        include_bytes!("../templates/ai-hook/stop/types.ts"),
    ),
    (
        "stop/support/vitest-coverage.ts",
        include_bytes!("../templates/ai-hook/stop/support/vitest-coverage.ts"),
    ),
];

const RUNNER_FILES: [(Check, &str, &[u8]); 2] = [
    (
        Check::Vitest,
        "stop/runner/vitest.ts",
        include_bytes!("../templates/ai-hook/stop/runner/vitest.ts"),
    ),
    (
        Check::Eslint,
        "stop/runner/eslint.ts",
        include_bytes!("../templates/ai-hook/stop/runner/eslint.ts"),
    ),
];

const OWNED_HANDLER_TOKENS: [&str; 8] = [
    PRE_TOOL_USE_FILE,
    POST_TOOL_USE_FILE,
    STOP_ENTRY_FILE,
    ".codex/hooks/jt-vitest/",
    ".codex/hooks/nlab-eslint/",
    ".codex/hooks/nlab-eslint-stop.ts",
    ".codex/hooks/nlab-eslint-stop.mjs",
    "jt __vitest-hook",
];

type WritePlan = (PathBuf, Option<Vec<u8>>, Vec<u8>);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, ValueEnum)]
pub enum Check {
    Vitest,
    Eslint,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, ValueEnum)]
pub enum Agent {
    Codex,
}

#[derive(Debug, clap::Args)]
pub struct AiHookArgs {
    /// Final enabled checks
    #[arg(long, value_enum, value_delimiter = ',', requires = "agents")]
    checks: Option<Vec<Check>>,
    /// Final enabled agent terminals
    #[arg(long, value_enum, value_delimiter = ',', requires = "checks")]
    agents: Option<Vec<Agent>>,
}

#[derive(Default)]
struct CurrentSelection {
    agents: BTreeSet<Agent>,
    checks: BTreeSet<Check>,
    configured: bool,
}

pub fn run(args: AiHookArgs) -> u8 {
    match configure(args) {
        Ok((changed, checks, agents)) => {
            let state = if changed {
                "configured"
            } else {
                "already configured"
            };
            println!(
                "{state} AI hooks: checks={}, agents={}; review and trust repository hooks with /hooks",
                names(&checks),
                names(&agents),
            );
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn configure(args: AiHookArgs) -> Result<(bool, BTreeSet<Check>, BTreeSet<Agent>), String> {
    let cwd =
        env::current_dir().map_err(|error| format!("cannot read current directory: {error}"))?;
    let root = git_root(&cwd)?;
    let current = detect_selection(&root)?;
    let (checks, agents) = match (args.checks, args.agents) {
        (Some(checks), Some(agents)) => (
            checks.into_iter().collect::<BTreeSet<_>>(),
            agents.into_iter().collect::<BTreeSet<_>>(),
        ),
        (None, None) => interactive_selection(&current)?,
        _ => unreachable!("clap requires --checks and --agents together"),
    };
    let changed = install(&root, &checks, &agents)?;
    Ok((changed, checks, agents))
}

fn interactive_selection(
    current: &CurrentSelection,
) -> Result<(BTreeSet<Check>, BTreeSet<Agent>), String> {
    if !std::io::stdin().is_terminal() {
        return Err(
            "interactive terminal required; use --checks vitest,eslint --agents codex".to_owned(),
        );
    }

    cliclack::intro("jt ai-hook").map_err(|error| format!("cannot start prompt: {error}"))?;
    let initial_checks = if current.configured {
        current.checks.iter().copied().collect()
    } else {
        vec![Check::Vitest, Check::Eslint]
    };
    let checks = cliclack::multiselect("选择校验内容")
        .item(Check::Vitest, "Vitest", "相关单测与覆盖率")
        .item(Check::Eslint, "ESLint", "AI 变更文件诊断")
        .initial_values(initial_checks)
        .required(false)
        .interact()
        .map_err(|error| format!("cannot read check selection: {error}"))?;
    let initial_agents = if current.configured {
        current.agents.iter().copied().collect()
    } else {
        vec![Agent::Codex]
    };
    let agents = cliclack::multiselect("选择 Agent 终端")
        .item(Agent::Codex, "Codex", "Claude Code 后续支持")
        .initial_values(initial_agents)
        .required(false)
        .interact()
        .map_err(|error| format!("cannot read agent selection: {error}"))?;
    Ok((checks.into_iter().collect(), agents.into_iter().collect()))
}

fn names<T>(values: &BTreeSet<T>) -> String
where
    T: ValueEnum + Copy + Ord,
{
    if values.is_empty() {
        return "none".to_owned();
    }
    values
        .iter()
        .filter_map(|value| value.to_possible_value())
        .map(|value| value.get_name().to_owned())
        .collect::<Vec<_>>()
        .join(",")
}

fn install(
    root: &Path,
    checks: &BTreeSet<Check>,
    agents: &BTreeSet<Agent>,
) -> Result<bool, String> {
    let custom_runner = has_custom_runner(root)?;
    let codex_enabled = agents.contains(&Agent::Codex) && (!checks.is_empty() || custom_runner);
    if !checks.is_empty() || codex_enabled {
        require_dependencies(root, checks, codex_enabled)?;
    }

    let mut writes = Vec::new();
    if !checks.is_empty() || codex_enabled {
        for (name, content) in COMMON_FILES {
            plan_owned_write(root, name, content, &mut writes)?;
        }
    }
    let mut removals = Vec::new();
    for (check, name, content) in RUNNER_FILES {
        if checks.contains(&check) {
            plan_owned_write(root, name, content, &mut writes)?;
        } else {
            plan_owned_removal(root, name, &mut removals)?;
        }
    }

    let config_path = root.join(".codex/hooks.json");
    let current_config = read_install_file(root, &config_path)?;
    let mut config = match current_config.as_deref() {
        Some(bytes) => serde_json::from_slice::<Value>(bytes)
            .map_err(|error| format!("invalid {}: {error}", config_path.display()))?,
        None => Value::Object(Map::new()),
    };
    let previous_config = config.clone();
    merge_hooks(&mut config, codex_enabled)?;
    if config != previous_config || (current_config.is_none() && codex_enabled) {
        let mut next = serde_json::to_vec_pretty(&config)
            .map_err(|error| format!("serialize {}: {error}", config_path.display()))?;
        next.push(b'\n');
        writes.push((config_path.clone(), current_config.clone(), next));
    }

    let changed = writes
        .iter()
        .any(|(_, current, next)| current.as_deref() != Some(next.as_slice()))
        || !removals.is_empty();
    for (path, current, next) in writes.iter().filter(|(path, _, _)| path != &config_path) {
        if current.as_deref() != Some(next.as_slice()) {
            atomic_write(root, path, current.as_deref(), next)
                .map_err(|error| error.to_string())?;
        }
    }
    for (path, expected) in removals {
        let current = fs::read(&path).map_err(|error| {
            format!("cannot re-read {} before removal: {error}", path.display())
        })?;
        if current != expected {
            return Err(format!(
                "refuse to remove concurrently changed hook runner: {}",
                path.display()
            ));
        }
        fs::remove_file(&path)
            .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
    }
    if let Some((path, current, next)) = writes.iter().find(|(path, _, _)| path == &config_path)
        && current.as_deref() != Some(next.as_slice())
    {
        atomic_write(root, path, current.as_deref(), next).map_err(|error| error.to_string())?;
    }
    Ok(changed)
}

fn plan_owned_write(
    root: &Path,
    name: &str,
    content: &[u8],
    writes: &mut Vec<WritePlan>,
) -> Result<(), String> {
    let path = root.join(HOOK_DIR).join(name);
    let current = read_install_file(root, &path)?;
    if let Some(bytes) = current.as_deref()
        && bytes != content
        && !String::from_utf8_lossy(bytes).contains(OWNED_MARKER)
    {
        return Err(format!(
            "refuse to replace unowned AI hook template: {}",
            path.display()
        ));
    }
    writes.push((path, current, content.to_vec()));
    Ok(())
}

fn plan_owned_removal(
    root: &Path,
    name: &str,
    removals: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<(), String> {
    let path = root.join(HOOK_DIR).join(name);
    let Some(current) = read_install_file(root, &path)? else {
        return Ok(());
    };
    if !String::from_utf8_lossy(&current).contains(OWNED_MARKER) {
        return Err(format!(
            "refuse to disable unowned AI hook runner: {}",
            path.display()
        ));
    }
    removals.push((path, current));
    Ok(())
}

fn detect_selection(root: &Path) -> Result<CurrentSelection, String> {
    let mut current = CurrentSelection::default();
    for (check, name, _) in RUNNER_FILES {
        if root.join(HOOK_DIR).join(name).exists() {
            current.checks.insert(check);
            current.configured = true;
        }
    }
    if root.join(".codex/hooks/jt-vitest").exists() {
        current.checks.insert(Check::Vitest);
        current.agents.insert(Agent::Codex);
        current.configured = true;
    }
    if root.join(".codex/hooks/nlab-eslint").exists() {
        current.checks.insert(Check::Eslint);
        current.agents.insert(Agent::Codex);
        current.configured = true;
    }

    let config_path = root.join(".codex/hooks.json");
    let config = read_install_file(root, &config_path)?;
    if let Some(bytes) = config {
        let text = String::from_utf8_lossy(&bytes);
        if text.contains(".codex/hooks/jt-vitest/") || text.contains("jt __vitest-hook") {
            current.checks.insert(Check::Vitest);
        }
        if text.contains(".codex/hooks/nlab-eslint") {
            current.checks.insert(Check::Eslint);
        }
        if OWNED_HANDLER_TOKENS
            .iter()
            .any(|token| text.contains(token))
        {
            current.agents.insert(Agent::Codex);
            current.configured = true;
        }
    }
    Ok(current)
}

fn has_custom_runner(root: &Path) -> Result<bool, String> {
    let directory = root.join(HOOK_DIR).join("stop/runner");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot inspect {}: {error}", directory.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect runner: {error}"))?;
        let name = entry.file_name();
        if name == "vitest.ts" || name == "eslint.ts" {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if file_type.is_file() && entry.path().extension().is_some_and(|value| value == "ts") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn require_dependencies(
    root: &Path,
    checks: &BTreeSet<Check>,
    codex_enabled: bool,
) -> Result<(), String> {
    let mut required = BTreeSet::new();
    if codex_enabled || !checks.is_empty() {
        required.insert("tsx");
    }
    if checks.contains(&Check::Vitest) {
        required.insert("vitest");
    }
    if checks.contains(&Check::Eslint) {
        required.insert("eslint");
    }
    if required.is_empty() {
        return Ok(());
    }

    let path = root.join("package.json");
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let package: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    for dependency in required {
        let declared = [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ]
        .iter()
        .any(|field| {
            package
                .get(*field)
                .and_then(Value::as_object)
                .is_some_and(|dependencies| dependencies.contains_key(dependency))
        });
        if !declared {
            return Err(format!(
                "root package.json does not declare {dependency}; add it before installing this hook"
            ));
        }
    }
    Ok(())
}

fn git_root(cwd: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("cannot run git to resolve repository root: {error}"))?;
    if !output.status.success() {
        return Err("current directory is not inside a Git repository".to_owned());
    }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if root.as_os_str().is_empty() {
        return Err("Git returned an empty repository root".to_owned());
    }
    root.canonicalize()
        .map_err(|error| format!("cannot resolve Git repository root: {error}"))
}

fn read_install_file(root: &Path, path: &Path) -> Result<Option<Vec<u8>>, String> {
    reject_symlink_path(root, path)?;
    read_optional(path).map_err(|error| error.to_string())
}

fn reject_symlink_path(root: &Path, path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "refuse to replace symlinked file: {}",
            path.display()
        ));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("hook path is outside Git root: {}", path.display()))?;
    let mut cursor = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        cursor.push(component.as_os_str());
        if fs::symlink_metadata(&cursor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(format!(
                "refuse to write through symlinked parent: {}",
                cursor.display()
            ));
        }
    }
    Ok(())
}

fn command_hook(script: &str, status: &str, timeout: u64) -> Value {
    serde_json::json!({
        "hooks": [{
            "command": format!(
                "pnpm --dir \"$(git rev-parse --show-toplevel)\" exec tsx {script} codex"
            ),
            "statusMessage": status,
            "timeout": timeout,
            "type": "command"
        }]
    })
}

fn merge_hooks(value: &mut Value, enabled: bool) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "existing .codex/hooks.json must contain a JSON object".to_owned())?;
    if !enabled && !object.contains_key("hooks") {
        return Ok(());
    }
    let hooks = object
        .entry("hooks".to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "existing .codex/hooks.json hooks must be an object".to_owned())?;

    let stages = [
        (
            "PreToolUse",
            with_matcher(
                command_hook(PRE_TOOL_USE_FILE, "Tracking AI edit", 30),
                "Edit|Write",
            ),
        ),
        (
            "PostToolUse",
            with_matcher(
                command_hook(POST_TOOL_USE_FILE, "Recording AI-edited files", 30),
                "Edit|Write",
            ),
        ),
        (
            "Stop",
            command_hook(STOP_ENTRY_FILE, "Running AI checks", 150),
        ),
    ];
    for (event, group) in stages {
        merge_hook_event(hooks, event, enabled.then_some(group))?;
    }
    Ok(())
}

fn with_matcher(mut group: Value, matcher: &str) -> Value {
    let mut output = Map::new();
    output.insert("matcher".to_owned(), Value::String(matcher.to_owned()));
    output.append(group.as_object_mut().expect("command hook is an object"));
    Value::Object(output)
}

fn merge_hook_event(
    hooks: &mut Map<String, Value>,
    event: &str,
    owned: Option<Value>,
) -> Result<(), String> {
    let Some(value) = hooks.get_mut(event) else {
        if let Some(group) = owned {
            hooks.insert(event.to_owned(), Value::Array(vec![group]));
        }
        return Ok(());
    };
    let groups = value
        .as_array_mut()
        .ok_or_else(|| format!("existing .codex/hooks.json hooks.{event} must be an array"))?;
    groups.retain_mut(retain_unowned_handlers);
    if let Some(group) = owned {
        groups.push(group);
    }
    Ok(())
}

fn retain_unowned_handlers(group: &mut Value) -> bool {
    let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
        return true;
    };
    handlers.retain(|handler| !handler_is_owned(handler));
    !handlers.is_empty()
}

fn handler_is_owned(handler: &Value) -> bool {
    let command_owned = handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(contains_owned_token);
    let args_owned = handler
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .any(contains_owned_token)
        });
    command_owned || args_owned
}

fn contains_owned_token(value: &str) -> bool {
    OWNED_HANDLER_TOKENS
        .iter()
        .any(|token| value.contains(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_merge_filters_owned_handlers_and_preserves_unrelated_handlers() {
        let mut config = serde_json::json!({
            "keep": true,
            "hooks": {
                "Stop": [
                    {"hooks": [
                        {"type": "command", "command": "echo keep"},
                        {"type": "command", "command": "pnpm exec tsx .codex/hooks/jt-vitest/stop.ts codex"}
                    ]},
                    {"hooks": [{"type": "command", "command": "pnpm exec tsx .codex/hooks/nlab-eslint/stop.ts codex"}]}
                ]
            }
        });

        merge_hooks(&mut config, true).unwrap();
        let once = config.clone();
        merge_hooks(&mut config, true).unwrap();

        assert_eq!(config, once);
        assert_eq!(config["keep"], true);
        assert_eq!(config["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(config["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(config["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(
            config["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo keep"
        );
        assert!(config.to_string().contains(STOP_ENTRY_FILE));
        assert!(!config.to_string().contains("jt-vitest"));
        assert!(!config.to_string().contains("nlab-eslint/stop"));
    }
}
