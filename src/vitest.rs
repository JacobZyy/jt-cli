use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Map, Value};

use crate::node::fs::{atomic_write, read_optional};

const HOOK_DIR: &str = ".codex/hooks/jt-vitest";
const PRE_TOOL_USE_FILE: &str = ".codex/hooks/jt-vitest/pre-tool-use.ts";
const POST_TOOL_USE_FILE: &str = ".codex/hooks/jt-vitest/post-tool-use.ts";
const STOP_FILE: &str = ".codex/hooks/jt-vitest/stop.ts";
const OWNED_MARKER: &str = "jt-vitest-ai-hook";
const LEGACY_COMMAND: &str = "jt __vitest-hook";

const RUNTIME_FILES: [(&str, &[u8]); 8] = [
    (
        "coverage.ts",
        include_bytes!("../templates/vitest-ai-hook/coverage.ts"),
    ),
    (
        "files.ts",
        include_bytes!("../templates/vitest-ai-hook/files.ts"),
    ),
    (
        "post-tool-use.ts",
        include_bytes!("../templates/vitest-ai-hook/post-tool-use.ts"),
    ),
    (
        "pre-tool-use.ts",
        include_bytes!("../templates/vitest-ai-hook/pre-tool-use.ts"),
    ),
    (
        "protocol.ts",
        include_bytes!("../templates/vitest-ai-hook/protocol.ts"),
    ),
    (
        "runtime.ts",
        include_bytes!("../templates/vitest-ai-hook/runtime.ts"),
    ),
    (
        "stop.ts",
        include_bytes!("../templates/vitest-ai-hook/stop.ts"),
    ),
    (
        "vitest.ts",
        include_bytes!("../templates/vitest-ai-hook/vitest.ts"),
    ),
];

pub fn install(target: &OsString) -> u8 {
    match target.to_string_lossy().as_ref() {
        "--codex" => match install_codex() {
            Ok(changed) => {
                println!(
                    "{} Vitest AI hooks; review and trust repository hooks with /hooks",
                    if changed {
                        "installed"
                    } else {
                        "already configured"
                    }
                );
                0
            }
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        },
        "--claude" => {
            eprintln!("error: Claude support is not implemented");
            1
        }
        _ => {
            eprintln!("error: usage: jt vitest ai-hook <--codex|--claude>");
            2
        }
    }
}

fn install_codex() -> Result<bool, String> {
    let cwd =
        env::current_dir().map_err(|error| format!("cannot read current directory: {error}"))?;
    let root = git_root(&cwd)?;
    require_vitest(&root)?;

    let mut writes = Vec::with_capacity(RUNTIME_FILES.len() + 1);
    for (name, content) in RUNTIME_FILES {
        let path = root.join(HOOK_DIR).join(name);
        let current = read_install_file(&root, &path)?;
        if let Some(bytes) = current.as_deref()
            && bytes != content
            && !String::from_utf8_lossy(bytes).contains(OWNED_MARKER)
        {
            return Err(format!(
                "refuse to replace unowned Vitest hook template: {}",
                path.display()
            ));
        }
        writes.push((path, current, content.to_vec()));
    }

    let config_path = root.join(".codex/hooks.json");
    let current_config = read_install_file(&root, &config_path)?;
    let mut config = match current_config.as_deref() {
        Some(bytes) => serde_json::from_slice::<Value>(bytes)
            .map_err(|error| format!("invalid {}: {error}", config_path.display()))?,
        None => Value::Object(Map::new()),
    };
    merge_hooks(&mut config)?;
    let mut next_config = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("serialize {}: {error}", config_path.display()))?;
    next_config.push(b'\n');
    writes.push((config_path, current_config, next_config));

    let changed = writes
        .iter()
        .any(|(_, current, next)| current.as_deref() != Some(next.as_slice()));
    for (path, current, next) in writes {
        if current.as_deref() != Some(next.as_slice()) {
            atomic_write(&root, &path, current.as_deref(), &next)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(changed)
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

fn require_vitest(root: &Path) -> Result<(), String> {
    let path = root.join("package.json");
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let package: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    for dependency in ["vitest", "tsx"] {
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

fn merge_hooks(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "existing .codex/hooks.json must contain a JSON object".to_owned())?;
    let hooks = object
        .entry("hooks".to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "existing .codex/hooks.json hooks must be an object".to_owned())?;

    let pre = with_matcher(
        command_hook(PRE_TOOL_USE_FILE, "Tracking Vitest AI edit", 30),
        "Edit|Write",
    );
    let post = with_matcher(
        command_hook(POST_TOOL_USE_FILE, "Recording Vitest AI-edited files", 30),
        "Edit|Write",
    );
    let stop = command_hook(STOP_FILE, "Running related Vitest suites", 150);
    merge_hook_group(hooks, "PostToolUse", post, &[POST_TOOL_USE_FILE])?;
    merge_hook_group(hooks, "PreToolUse", pre, &[PRE_TOOL_USE_FILE])?;
    merge_hook_group(hooks, "Stop", stop, &[STOP_FILE, LEGACY_COMMAND])?;
    Ok(())
}

fn with_matcher(mut group: Value, matcher: &str) -> Value {
    let mut output = Map::new();
    output.insert("matcher".to_owned(), Value::String(matcher.to_owned()));
    output.append(group.as_object_mut().expect("command hook is an object"));
    Value::Object(output)
}

fn merge_hook_group(
    hooks: &mut Map<String, Value>,
    event: &str,
    owned: Value,
    script_paths: &[&str],
) -> Result<(), String> {
    let groups = hooks
        .entry(event.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| format!("existing .codex/hooks.json hooks.{event} must be an array"))?;
    if let Some(index) = groups
        .iter()
        .position(|group| hook_group_uses_script(group, script_paths))
    {
        groups[index] = owned;
    } else {
        groups.push(owned);
    }
    Ok(())
}

fn hook_group_uses_script(group: &Value, script_paths: &[&str]) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|handlers| {
            handlers.iter().any(|handler| {
                handler
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| script_paths.iter().any(|path| command.contains(path)))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_merge_installs_three_stages_and_migrates_legacy_stop() {
        let mut config = serde_json::json!({
            "keep": true,
            "hooks": {
                "Stop": [
                    {"hooks": [{"type": "command", "command": "echo keep"}]},
                    {"label": "legacy", "hooks": [{"type": "command", "command": LEGACY_COMMAND}]}
                ]
            }
        });

        merge_hooks(&mut config).unwrap();
        let once = config.clone();
        merge_hooks(&mut config).unwrap();

        assert_eq!(config, once);
        assert_eq!(config["keep"], true);
        assert_eq!(config["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(config["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(config["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(
            config["hooks"]["Stop"][1]["hooks"][0]["command"],
            format!("pnpm --dir \"$(git rev-parse --show-toplevel)\" exec tsx {STOP_FILE} codex")
        );
        assert!(config.to_string().contains(PRE_TOOL_USE_FILE));
        assert!(config.to_string().contains(POST_TOOL_USE_FILE));
    }
}
