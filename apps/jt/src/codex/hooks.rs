use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, ObjectPropertyKind, Statement};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use serde_json::{Value, json};

use super::shell_quote;

const TASK: &str = "jt-worktree-inherit";
const JS_BEGIN: &str = "// JT_WORKTREE_INHERIT_BEGIN";
const JS_END: &str = "// JT_WORKTREE_INHERIT_END";
const JS_HELPER: &str = "__jtWorktreeInherit";
const SHELL_MARKER: &str = "# jt-worktree-inherit original=";
const SIMPLE_CONFIGS: &[&str] = &[
    ".simple-git-hooks.cjs",
    ".simple-git-hooks.js",
    ".simple-git-hooks.mjs",
    "simple-git-hooks.cjs",
    "simple-git-hooks.js",
    "simple-git-hooks.mjs",
    ".simple-git-hooks.json",
    "simple-git-hooks.json",
];
const LEFTHOOK_CONFIGS: &[&str] = &[
    "lefthook.yml",
    "lefthook.yaml",
    ".lefthook.yml",
    ".lefthook.yaml",
    ".config/lefthook.yml",
    ".config/lefthook.yaml",
    "lefthook.toml",
    ".lefthook.toml",
    ".config/lefthook.toml",
    "lefthook.json",
    ".lefthook.json",
    ".config/lefthook.json",
    "lefthook.jsonc",
    ".lefthook.jsonc",
    ".config/lefthook.jsonc",
];
const LEFTHOOK_LOCAL: &[&str] = &[
    "lefthook-local.yml",
    "lefthook-local.yaml",
    ".lefthook-local.yml",
    ".lefthook-local.yaml",
    ".config/lefthook-local.yml",
    ".config/lefthook-local.yaml",
    "lefthook-local.toml",
    ".lefthook-local.toml",
    ".config/lefthook-local.toml",
    "lefthook-local.json",
    ".lefthook-local.json",
    ".config/lefthook-local.json",
    "lefthook-local.jsonc",
    ".lefthook-local.jsonc",
    ".config/lefthook-local.jsonc",
];

pub struct Report {
    pub messages: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Manager {
    Simple,
    Lefthook,
}

impl Manager {
    fn name(self) -> &'static str {
        match self {
            Self::Simple => "simple-git-hooks",
            Self::Lefthook => "lefthook",
        }
    }
}

pub fn local_overrides(root: &Path) -> Result<Vec<&'static str>, String> {
    let root = primary(root)?.unwrap_or_else(|| root.to_path_buf());
    let package = package(&root)?;
    if lefthook_configured(&root)
        || dependency(&package, "lefthook")
        || dependency(&package, "@evilmartians/lefthook")
    {
        Ok(LEFTHOOK_LOCAL.to_vec())
    } else {
        Ok(Vec::new())
    }
}

pub fn configure(root: &Path) -> Result<Report, String> {
    let mut messages = Vec::new();
    let Some(primary) = primary(root)? else {
        return Ok(disabled("project is not a Git worktree"));
    };
    let package = package(&primary)?;
    let simple = SIMPLE_CONFIGS
        .iter()
        .any(|name| primary.join(name).is_file())
        || package.get("simple-git-hooks").is_some()
        || script_mentions(&package, "simple-git-hooks");
    let lefthook = lefthook_configured(&primary) || script_mentions(&package, "lefthook");
    let manager = match (simple, lefthook) {
        (true, true) => {
            return Ok(disabled(
                "conflicting Lefthook and simple-git-hooks configurations; neither manager installed",
            ));
        }
        (true, false) => Manager::Simple,
        (false, true) => Manager::Lefthook,
        (false, false) => match (
            dependency(&package, "simple-git-hooks"),
            dependency(&package, "lefthook") || dependency(&package, "@evilmartians/lefthook"),
        ) {
            (true, true) => {
                return Ok(disabled(
                    "both hook managers are dependencies; select one configuration before installing",
                ));
            }
            (true, false) => Manager::Simple,
            (false, true) => Manager::Lefthook,
            (false, false) => {
                return Ok(disabled(
                    "no supported hook manager configuration or dependency; no dependency added",
                ));
            }
        },
    };
    let hooks = git_path(
        &primary,
        &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
    )?;
    if let Some(reason) = foreign_hooks(&hooks, manager)? {
        return Ok(disabled(&reason));
    }
    let command = "jt codex inherit --post-checkout";
    let config = match manager {
        Manager::Simple => update_simple(&primary, &package, command)?,
        Manager::Lefthook => update_lefthook(&primary, command)?,
    };
    messages.push(format!(
        "updated {} for {TASK}; commit shared hook configuration",
        config.display()
    ));
    if manager == Manager::Simple && simple_hooks_path(&primary)? != hooks {
        messages.push("原生 Git 自动继承尚未启用: simple-git-hooks installation path differs from Git's effective hooks directory; core.hooksPath was not changed".to_owned());
        return Ok(Report { messages });
    }
    let Some(binary) = binary(&primary, manager.name()) else {
        messages.push(format!("原生 Git 自动继承尚未启用: {} is unavailable; install project dependencies, then rerun jt codex init", manager.name()));
        return Ok(Report { messages });
    };
    if manager == Manager::Lefthook {
        let effective = Command::new(&binary)
            .args(["dump", "--format", "json"])
            .current_dir(&primary)
            .output()
            .map_err(|error| format!("cannot inspect effective Lefthook config: {error}"))?;
        let parsed = serde_json::from_slice::<Value>(&effective.stdout);
        if !effective.status.success()
            || !parsed.as_ref().is_ok_and(|config| {
                let hook = &config["post-checkout"];
                let task = &hook["commands"][TASK];
                hook.get("skip").and_then(Value::as_bool) != Some(true)
                    && task.get("skip").and_then(Value::as_bool) != Some(true)
                    && task.get("run").and_then(Value::as_str).is_some_and(|run| {
                        run == format!("{command} \"{{1}}\" \"{{2}}\" \"{{3}}\"")
                    })
            })
        {
            messages.push(format!("原生 Git 自动继承尚未启用: effective Lefthook config does not contain the JT inheritance task; inspect extends/local overrides: {}", String::from_utf8_lossy(&effective.stderr).trim()));
            return Ok(Report { messages });
        }
    }
    if manager == Manager::Simple
        && std::env::var("SKIP_INSTALL_SIMPLE_GIT_HOOKS")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true"))
    {
        messages.push("原生 Git 自动继承尚未启用: SKIP_INSTALL_SIMPLE_GIT_HOOKS disables manager installation".to_owned());
        return Ok(Report { messages });
    }
    let mut install = Command::new(&binary);
    install.current_dir(&primary);
    if manager == Manager::Lefthook {
        install.args(["install", "post-checkout"]);
    } else if config != primary.join("package.json") {
        install.arg(&config);
    }
    let output = match install.output() {
        Ok(output) => output,
        Err(error) => {
            messages.push(format!(
                "原生 Git 自动继承尚未启用: cannot run {}: {error}",
                binary.display()
            ));
            return Ok(Report { messages });
        }
    };
    if !output.status.success() {
        messages.push(format!(
            "原生 Git 自动继承尚未启用: {} install failed: {}{}",
            manager.name(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        return Ok(Report { messages });
    }
    let actual = git_path(
        &primary,
        &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
    )?;
    let hook = actual.join("post-checkout");
    let installed = fs::read_to_string(&hook).ok();
    let valid = actual == hooks
        && installed.as_ref().is_some_and(|content| match manager {
            Manager::Simple => {
                content.contains(SHELL_MARKER)
                    && content.contains(command)
                    && content.contains("\"$@\"")
            }
            Manager::Lefthook => {
                content.contains("lefthook")
                    && content.contains("post-checkout")
                    && has_absolute_lefthook(content)
            }
        })
        && executable_file(&hook);
    if valid {
        messages.push(format!(
            "enabled native Git local configuration inheritance via {} at {} (installed from {})",
            manager.name(),
            hook.display(),
            primary.display()
        ));
    } else {
        messages.push(format!(
            "原生 Git 自动继承尚未启用: {} did not install a usable post-checkout at {}",
            manager.name(),
            hook.display()
        ));
    }
    Ok(Report { messages })
}

fn disabled(reason: &str) -> Report {
    Report {
        messages: vec![format!("原生 Git 自动继承尚未启用: {reason}")],
    }
}

fn package(root: &Path) -> Result<Value, String> {
    match fs::read(root.join("package.json")) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| format!("invalid package.json: {error}"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error.to_string()),
    }
}

fn dependency(package: &Value, name: &str) -> bool {
    ["dependencies", "devDependencies", "optionalDependencies"]
        .iter()
        .any(|key| {
            package
                .get(key)
                .and_then(|dependencies| dependencies.get(name))
                .is_some()
        })
}

fn script_mentions(package: &Value, name: &str) -> bool {
    package
        .get("scripts")
        .and_then(Value::as_object)
        .is_some_and(|scripts| {
            scripts.values().filter_map(Value::as_str).any(|script| {
                shell_words(script)
                    .iter()
                    .any(|word| Path::new(word).file_name().is_some_and(|file| file == name))
            })
        })
}

fn lefthook_configured(root: &Path) -> bool {
    std::env::var_os("LEFTHOOK_CONFIG").is_some()
        || LEFTHOOK_CONFIGS
            .iter()
            .chain(LEFTHOOK_LOCAL)
            .any(|name| root.join(name).is_file())
}

fn primary(root: &Path) -> Result<Option<PathBuf>, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    let output = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    Ok(output
        .split('\0')
        .next()
        .and_then(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from))
}

fn git_path(root: &Path, args: &[&str]) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(PathBuf::from(
        String::from_utf8(output.stdout)
            .map_err(|error| error.to_string())?
            .trim_end_matches(['\r', '\n']),
    ))
}

fn foreign_hooks(directory: &Path, manager: Manager) -> Result<Option<String>, String> {
    if !directory.exists() {
        return Ok(None);
    }
    if fs::symlink_metadata(directory)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Ok(Some(format!(
            "refusing symlinked hooks directory {}",
            directory.display()
        )));
    }
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_symlink()
        {
            return Ok(Some(format!(
                "refusing symlinked hook {}",
                entry.path().display()
            )));
        }
        if entry.file_name().to_string_lossy().ends_with(".sample") || !entry.path().is_file() {
            continue;
        }
        let content = fs::read_to_string(entry.path()).unwrap_or_default();
        let foreign = match manager {
            Manager::Simple => content.contains("lefthook") || content.contains("husky"),
            Manager::Lefthook => {
                content.contains("SKIP_SIMPLE_GIT_HOOKS") || content.contains("husky")
            }
        };
        if foreign
            || (entry.file_name() == "post-checkout"
                && !content.contains("SKIP_SIMPLE_GIT_HOOKS")
                && !content.contains("lefthook"))
        {
            return Ok(Some(format!(
                "existing hook {} belongs to another manager or has unmanaged business logic; not overwritten",
                entry.path().display()
            )));
        }
    }
    Ok(None)
}

fn simple_hooks_path(root: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["config", "--local", "core.hooksPath"])
        .current_dir(root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(root.join(".git/hooks"));
    }
    let path = PathBuf::from(
        String::from_utf8(output.stdout)
            .map_err(|error| error.to_string())?
            .trim_end_matches(['\r', '\n']),
    );
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn binary(root: &Path, name: &str) -> Option<PathBuf> {
    std::iter::once(root.join("node_modules/.bin").join(name))
        .chain(std::env::var_os("PATH").into_iter().flat_map(|paths| {
            std::env::split_paths(&paths)
                .map(|directory| directory.join(name))
                .collect::<Vec<_>>()
        }))
        .find(|path| executable_file(path))
}

fn executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

fn has_absolute_lefthook(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line
            .trim()
            .trim_start_matches("elif ")
            .trim_start_matches(['\'', '"']);
        line.starts_with('/') && line.contains("lefthook")
    })
}

fn update_simple(root: &Path, package: &Value, command: &str) -> Result<PathBuf, String> {
    let custom = simple_custom_config(package)?;
    let candidates: Vec<_> = custom
        .into_iter()
        .chain(SIMPLE_CONFIGS.iter().map(|name| (*name).to_owned()))
        .collect();
    let path = effective_simple_source(root, &candidates, package)?;
    ensure_regular(root, &path)?;
    if path
        .extension()
        .is_some_and(|extension| matches!(extension.to_str(), Some("js" | "cjs" | "mjs")))
    {
        let content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        write_if_changed(&path, &update_javascript(&content, &path, command)?)?;
    } else {
        let mut content = if path.is_file() {
            serde_json::from_slice::<Value>(&fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("invalid {}: {error}", path.display()))?
        } else {
            json!({})
        };
        let hooks = if path == root.join("package.json") {
            let object = content
                .as_object_mut()
                .ok_or("package.json must be an object")?;
            object
                .entry("simple-git-hooks")
                .or_insert_with(|| json!({}))
        } else {
            &mut content
        };
        let hooks = hooks
            .as_object_mut()
            .ok_or("simple-git-hooks configuration must be an object")?;
        let original = hooks
            .get("post-checkout")
            .map(|value| {
                value
                    .as_str()
                    .ok_or("post-checkout command must be a string")
            })
            .transpose()?
            .unwrap_or_default();
        hooks.insert(
            "post-checkout".to_owned(),
            Value::String(chain_command(original, command)?),
        );
        hooks.insert("preserveUnused".to_owned(), Value::Bool(true));
        write_if_changed(
            &path,
            &format!("{}\n", serde_json::to_string_pretty(&content).unwrap()),
        )?;
    }
    Ok(path)
}

fn effective_simple_source(
    root: &Path,
    candidates: &[String],
    package: &Value,
) -> Result<PathBuf, String> {
    const PROBE: &str = r#"
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
const root = process.argv[1];
for (const name of JSON.parse(process.argv[2])) {
  const file = path.resolve(root, name);
  let config;
  try {
    if (file.endsWith('.json')) config = JSON.parse(fs.readFileSync(file, 'utf8'));
    else { const imported = await import(pathToFileURL(file)); config = imported.default || imported; }
  } catch { continue; }
  if (config) { process.stdout.write('\nJT_SIMPLE_SOURCE=' + JSON.stringify(file) + '\n'); process.exit(0); }
}
process.stdout.write('\nJT_SIMPLE_SOURCE=null\n');
"#;
    let existing: Vec<_> = candidates
        .iter()
        .filter(|name| root.join(name).is_file())
        .collect();
    for name in &existing {
        ensure_regular(root, &root.join(name))?;
    }
    if existing.iter().any(|name| !name.ends_with(".json")) {
        let output = Command::new("node")
            .args(["--input-type=module", "-e", PROBE])
            .arg(root)
            .arg(serde_json::to_string(candidates).unwrap())
            .current_dir(root)
            .output()
            .map_err(|error| {
                format!("cannot resolve effective simple-git-hooks JavaScript config: {error}")
            })?;
        if !output.status.success() {
            return Err(format!(
                "cannot resolve effective simple-git-hooks config: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let stdout = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
        let resolved = stdout
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("JT_SIMPLE_SOURCE="))
            .ok_or("simple-git-hooks config resolver produced no result")?;
        if let Some(path) =
            serde_json::from_str::<Option<String>>(resolved).map_err(|error| error.to_string())?
        {
            return Ok(PathBuf::from(path));
        }
    } else {
        for name in existing {
            let path = root.join(name);
            if serde_json::from_slice::<Value>(&fs::read(&path).map_err(|error| error.to_string())?)
                .is_ok_and(|value| match value {
                    Value::Null => false,
                    Value::Bool(value) => value,
                    Value::Number(value) => value.as_f64() != Some(0.0),
                    Value::String(value) => !value.is_empty(),
                    _ => true,
                })
            {
                return Ok(path);
            }
        }
    }
    if let Some(path) = package.get("simple-git-hooks").and_then(Value::as_str) {
        let path = root.join(path);
        ensure_regular(root, &path)?;
        if !path.is_file() {
            return Err(format!(
                "simple-git-hooks config {} does not exist",
                path.display()
            ));
        }
        return Ok(path);
    }
    Ok(root.join("package.json"))
}

fn simple_custom_config(package: &Value) -> Result<Option<String>, String> {
    let mut paths = Vec::new();
    if let Some(scripts) = package.get("scripts").and_then(Value::as_object) {
        for script in scripts.values().filter_map(Value::as_str) {
            let words = shell_words(script);
            for pair in words.windows(2) {
                if Path::new(&pair[0])
                    .file_name()
                    .is_some_and(|file| file == "simple-git-hooks")
                    && !matches!(pair[1].as_str(), "&&" | "&" | ";" | "||" | "|")
                {
                    paths.push(pair[1].clone());
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    match paths.as_slice() {
        [] => Ok(None),
        [path] if !Path::new(path).is_absolute() && !Path::new(path).components().any(|component| matches!(component, std::path::Component::ParentDir)) => Ok(Some(path.clone())),
        _ => Err("ambiguous or external simple-git-hooks configuration path; configure one project-local source".to_owned()),
    }
}

fn shell_words(script: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in script.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                word.push(ch);
            }
        } else if quote.is_none() && matches!(ch, ';' | '|' | '&' | '\n') {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            words.push(ch.to_string());
        } else if ch.is_whitespace() && quote.is_none() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn chain_command(original: &str, command: &str) -> Result<String, String> {
    let original = if let Some(managed) = original.strip_prefix(SHELL_MARKER) {
        serde_json::from_str::<String>(managed.lines().next().ok_or("invalid JT hook metadata")?)
            .map_err(|error| format!("invalid JT hook metadata: {error}"))?
    } else {
        original.to_owned()
    };
    Ok(format!(
        "{SHELL_MARKER}{}\nsh -c {} \"$0\" \"$@\"\njt_business_status=$?\n{command} \"$@\"\njt_inherit_status=$?\nif [ \"$jt_business_status\" -ne 0 ]; then exit \"$jt_business_status\"; fi\nexit \"$jt_inherit_status\"",
        serde_json::to_string(&original).unwrap(),
        shell_quote(&original)
    ))
}

fn update_javascript(content: &str, path: &Path, command: &str) -> Result<String, String> {
    if !content.contains(JS_BEGIN) && content.contains(JS_HELPER) {
        return Err(format!(
            "{} already uses reserved JT hook helper {JS_HELPER}",
            path.display()
        ));
    }
    let mut source = strip_managed(content, JS_BEGIN, JS_END)?;
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        &source,
        SourceType::from_path(path)
            .map_err(|error| error.to_string())?
            .with_module(true),
    )
    .parse();
    if !parsed.errors.is_empty() {
        return Err(format!(
            "cannot parse {}: {}",
            path.display(),
            parsed.errors[0]
        ));
    }
    let module_syntax = parsed.program.body.iter().any(|statement| {
        matches!(
            statement,
            Statement::ImportDeclaration(_)
                | Statement::ExportNamedDeclaration(_)
                | Statement::ExportAllDeclaration(_)
        )
    });
    let mut esm = false;
    for statement in &parsed.program.body {
        if let Statement::ExportDefaultDeclaration(export) = statement {
            let expression = export
                .declaration
                .as_expression()
                .ok_or("simple-git-hooks default export must be a configuration expression")?;
            let span = expression.span();
            let original = &source[span.start as usize..span.end as usize];
            if !original.starts_with(&format!("{JS_HELPER}(")) {
                let replacement = format!("{JS_HELPER}({original})");
                source.replace_range(span.start as usize..span.end as usize, &replacement);
            }
            esm = true;
            break;
        }
        if let Statement::ExportNamedDeclaration(export) = statement {
            if let Some(specifier) = export
                .specifiers
                .iter()
                .find(|specifier| specifier.exported.name() == "default")
            {
                if export.source.is_some() {
                    return Err("simple-git-hooks default re-export must first be imported into the project config".to_owned());
                }
                const DEFAULT: &str = "__jtWorktreeInheritanceDefault";
                if specifier.local.name() != DEFAULT {
                    let span = specifier.local.span();
                    let local = specifier.local.name().to_string();
                    let insertion = export.span.start as usize;
                    source.replace_range(span.start as usize..span.end as usize, DEFAULT);
                    source.insert_str(
                        insertion,
                        &format!("const {DEFAULT} = {JS_HELPER}({local});\n"),
                    );
                }
                esm = true;
                break;
            }
        }
    }
    let package_module = path
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .find(|root| root.join("package.json").is_file())
        .map(package)
        .transpose()?
        .is_some_and(|package| package.get("type").and_then(Value::as_str) == Some("module"));
    if !esm
        && (path.extension().is_some_and(|extension| extension == "mjs")
            || package_module
            || module_syntax)
    {
        return Err(format!(
            "{} must export its configuration with export default",
            path.display()
        ));
    }
    let template = chain_command("", command)?;
    // Build the command at load time, retaining dynamic config expressions in their source.
    let body = template.split_once('\n').unwrap().1;
    let before = body.find("sh -c ''").ok_or("invalid shell template")?;
    let after = &body[before + "sh -c ''".len()..];
    let trailer = format!(
        "{JS_BEGIN}\nfunction {JS_HELPER}(config) {{\n  const original = config['post-checkout'] ?? '';\n  if (typeof original !== 'string') throw new TypeError('post-checkout command must be a string');\n  const quote = value => \"'\" + value.replaceAll(\"'\", \"'\\\"'\\\"'\") + \"'\";\n  return {{ ...config, preserveUnused: true, 'post-checkout': {} + JSON.stringify(original) + '\\nsh -c ' + quote(original) + {} }};\n}}\n{}{JS_END}\n",
        serde_json::to_string(SHELL_MARKER).unwrap(),
        serde_json::to_string(after).unwrap(),
        if esm {
            String::new()
        } else {
            format!("module.exports = {JS_HELPER}(module.exports);\n")
        }
    );
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(&trailer);
    Ok(source)
}

fn strip_managed(source: &str, begin: &str, end: &str) -> Result<String, String> {
    let Some(start) = source.find(begin) else {
        return Ok(source.to_owned());
    };
    let end = source[start..]
        .find(end)
        .ok_or("unterminated JT managed hook block")?
        + start
        + end.len();
    if source[end..].contains(begin) {
        return Err("multiple JT managed hook blocks".to_owned());
    }
    let line_start = source[..start].rfind('\n').map_or(0, |newline| newline + 1);
    let start = if source[line_start..start].trim().is_empty() {
        line_start
    } else {
        start
    };
    let end = end + usize::from(source[end..].starts_with('\n'));
    Ok(format!("{}{}", &source[..start], &source[end..]))
}

fn update_lefthook(root: &Path, command: &str) -> Result<PathBuf, String> {
    let configs: Vec<_> = LEFTHOOK_CONFIGS
        .iter()
        .filter(|name| root.join(name).is_file())
        .collect();
    let path = if let Some(path) = std::env::var_os("LEFTHOOK_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    } else {
        match configs.as_slice() {
        [] => {
            let local = LEFTHOOK_LOCAL.iter().find(|name| root.join(name).is_file());
            root.join(local.map_or_else(|| "lefthook.yml".to_owned(), |name| name.replace("-local", "")))
        },
        [name] => root.join(name),
        _ => return Err("multiple Lefthook main configurations; keep one effective source before initializing".to_owned()),
    }
    };
    ensure_regular(root, &path)?;
    let source = if path.exists() {
        fs::read_to_string(&path).map_err(|error| error.to_string())?
    } else {
        String::new()
    };
    let run = format!("{command} \"{{1}}\" \"{{2}}\" \"{{3}}\"");
    let source = match path.extension().and_then(|extension| extension.to_str()) {
        Some("json" | "jsonc") => {
            let source = if source.trim().is_empty() {
                "{}"
            } else {
                &source
            };
            json_set(
                source,
                &["post-checkout", "commands", TASK],
                &json!({"run": run}),
            )?
        }
        Some("toml") => update_toml(&source, &run)?,
        _ => update_yaml(&source, &run)?,
    };
    write_if_changed(&path, &source)?;
    Ok(path)
}

fn json_set(source: &str, keys: &[&str], value: &Value) -> Result<String, String> {
    let wrapped = format!("({source})");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &wrapped, SourceType::mjs()).parse();
    if !parsed.errors.is_empty() {
        return Err(format!(
            "cannot parse hook configuration: {}",
            parsed.errors[0]
        ));
    }
    let Some(Statement::ExpressionStatement(statement)) = parsed.program.body.first() else {
        return Err("hook configuration must be an object".to_owned());
    };
    let (start, end, replacement) = json_edit(&statement.expression, keys, value)?;
    let mut result = source.to_owned();
    result.replace_range(start - 1..end - 1, &replacement);
    Ok(result)
}

fn json_edit(
    expression: &Expression<'_>,
    keys: &[&str],
    value: &Value,
) -> Result<(usize, usize, String), String> {
    let expression = expression.without_parentheses();
    let Expression::ObjectExpression(object) = expression else {
        return Err("hook configuration section must be an object".to_owned());
    };
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return Err("JSON hook configuration cannot contain spread properties".to_owned());
        };
        if property
            .key
            .static_name()
            .is_some_and(|name| name == keys[0])
        {
            if keys.len() > 1 {
                return json_edit(&property.value, &keys[1..], value);
            }
            let span = property.value.span();
            return Ok((span.start as usize, span.end as usize, value.to_string()));
        }
    }
    let nested = keys[1..]
        .iter()
        .rev()
        .fold(value.clone(), |value, key| json!({*key: value}));
    let offset = object.span.start as usize + 1;
    Ok((
        offset,
        offset,
        format!(
            "\n{}: {}{}\n",
            serde_json::to_string(keys[0]).unwrap(),
            nested,
            if object.properties.is_empty() {
                ""
            } else {
                ","
            }
        ),
    ))
}

fn update_toml(source: &str, run: &str) -> Result<String, String> {
    let header = format!("[post-checkout.commands.{TASK}]");
    let mut source = source.to_owned();
    if let Some(start) = source.find(&header) {
        let end = source[start + header.len()..]
            .find("\n[")
            .map_or(source.len(), |offset| start + header.len() + offset + 1);
        source.replace_range(start..end, "");
    }
    source.truncate(source.trim_end_matches('\n').len());
    source.push_str(&format!(
        "\n\n{header}\nrun = {}\n",
        serde_json::to_string(run).unwrap()
    ));
    Ok(source)
}

fn yaml_key(line: &str) -> Option<(usize, &str, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if trimmed.starts_with('#') || trimmed.is_empty() {
        return None;
    }
    let (key, value) = trimmed.split_once(':')?;
    Some((
        line.len() - trimmed.len(),
        key.trim().trim_matches(['\'', '"']),
        value.trim(),
    ))
}

fn yaml_block_header(line: &str) -> String {
    let (key, value) = line.trim_end_matches('\n').split_once(':').unwrap();
    let comment = value.find('#').map(|start| &value[start..]);
    if value.trim().is_empty() || value.trim().starts_with('#') || value.trim().starts_with('&') {
        return format!("{}\n", line.trim_end_matches('\n'));
    }
    format!(
        "{key}:{}\n",
        comment.map_or_else(String::new, |comment| format!(" {comment}"))
    )
}

fn yaml_block_value(value: &str) -> bool {
    let scalar = value.split('#').next().unwrap().trim();
    matches!(scalar, "" | "{}" | "null" | "~")
        || (scalar.starts_with('&') && !scalar.contains(['{', '[']))
}

fn update_yaml(source: &str, run: &str) -> Result<String, String> {
    const BEGIN: &str = "# JT_WORKTREE_INHERIT_BEGIN";
    const END: &str = "# JT_WORKTREE_INHERIT_END";
    let mut source = strip_managed(source, BEGIN, END)?;
    let mut offset = 0;
    let lines: Vec<_> = source
        .split_inclusive('\n')
        .map(|line| {
            let current = offset;
            offset += line.len();
            (current, line)
        })
        .collect();
    let post = lines.iter().position(|(_, line)| {
        yaml_key(line).is_some_and(|(indent, key, _)| indent == 0 && key == "post-checkout")
    });
    let managed = |indent: usize| {
        format!(
            "{space}{BEGIN}\n{space}{TASK}:\n{space}  run: {}\n{space}{END}\n",
            serde_json::to_string(run).unwrap(),
            space = " ".repeat(indent)
        )
    };
    let Some(post) = post else {
        if !source.is_empty() && !source.ends_with('\n') {
            source.push('\n');
        }
        source.push_str(&format!("post-checkout:\n  commands:\n{}", managed(4)));
        return Ok(source);
    };
    let (post_offset, post_line) = lines[post];
    let (_, _, value) = yaml_key(post_line).unwrap();
    if !yaml_block_value(value) {
        return Err("Lefthook post-checkout must use YAML block mapping; convert its flow mapping before initializing".to_owned());
    }
    let end = lines
        .iter()
        .enumerate()
        .skip(post + 1)
        .find(|(_, (_, line))| yaml_key(line).is_some_and(|(indent, _, _)| indent == 0))
        .map_or(lines.len(), |(index, _)| index);
    let child_indent = lines[post + 1..end]
        .iter()
        .filter_map(|(_, line)| yaml_key(line).map(|(indent, _, _)| indent))
        .filter(|indent| *indent > 0)
        .min()
        .unwrap_or(2);
    let commands = lines[post + 1..end].iter().find(|(_, line)| {
        yaml_key(line).is_some_and(|(indent, key, _)| indent == child_indent && key == "commands")
    });
    if let Some((commands_offset, commands_line)) = commands {
        let (_, _, value) = yaml_key(commands_line).unwrap();
        if !yaml_block_value(value) {
            return Err("Lefthook commands must use YAML block mapping; convert its flow mapping before initializing".to_owned());
        }
        let insertion = commands_offset + commands_line.len();
        let command_indent = lines
            .iter()
            .filter(|(offset, _)| *offset >= insertion)
            .take_while(|(_, line)| {
                yaml_key(line).is_none_or(|(indent, _, _)| indent > child_indent)
            })
            .filter_map(|(_, line)| yaml_key(line).map(|(indent, _, _)| indent))
            .min()
            .unwrap_or(child_indent + 2);
        let task = lines
            .iter()
            .enumerate()
            .skip(post + 1)
            .take(end - post - 1)
            .find(|(_, (_, line))| {
                yaml_key(line)
                    .is_some_and(|(indent, key, _)| indent == command_indent && key == TASK)
            });
        if let Some((index, (start, _))) = task {
            let end = lines
                .iter()
                .skip(index + 1)
                .find(|(_, line)| {
                    yaml_key(line).is_some_and(|(indent, _, _)| indent <= command_indent)
                })
                .map_or(source.len(), |(offset, _)| *offset);
            source.replace_range(*start..end, "");
            return update_yaml(&source, run);
        }
        let replacement = format!(
            "{}{}",
            yaml_block_header(commands_line),
            managed(command_indent)
        );
        source.replace_range(*commands_offset..insertion, &replacement);
    } else {
        let replacement = format!(
            "{}{}commands:\n{}",
            yaml_block_header(post_line),
            " ".repeat(child_indent),
            managed(child_indent + 2)
        );
        source.replace_range(post_offset..post_offset + post_line.len(), &replacement);
    }
    Ok(source)
}

fn ensure_regular(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "hook config must be inside project")?;
    if relative.components().any(|component| {
        !matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    }) {
        return Err("hook config must be inside project".to_owned());
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing symlinked hook configuration {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    let current = match fs::read(path) {
        Ok(current) => Some(current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if current.as_deref() == Some(content.as_bytes()) {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path).ok();
    if metadata
        .as_ref()
        .is_some_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(format!(
            "refusing non-regular hook configuration {}",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .ok_or("hook configuration has no parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    if let Some(metadata) = metadata {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|error| error.to_string())?;
    }
    temporary
        .write_all(content.as_bytes())
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    if fs::read(path).ok() != current {
        return Err(format!(
            "hook configuration concurrently changed: {}",
            path.display()
        ));
    }
    temporary
        .persist(path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_chain_preserves_arguments_exit_exec_and_failure() {
        let temporary = tempfile::tempdir().unwrap();
        for (business, expected) in [("exit 17", 17), ("exec sh -c 'exit 19'", 19), ("exit 0", 0)] {
            let log = temporary.path().join("business and inheritance.log");
            let original = format!(
                "printf '%s\\n' \"$0\" \"$@\" > {}; {business}",
                shell_quote(&log.to_string_lossy())
            );
            let command = format!(
                "printf 'inherit:%s:%s:%s\\n' >> {}",
                shell_quote(&log.to_string_lossy())
            );
            let chained = chain_command(&original, &command).unwrap();
            let output = Command::new("sh")
                .args(["-c", &chained, "post-checkout", "old", "new", "1"])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(expected));
            let log = fs::read_to_string(&log).unwrap();
            assert!(log.starts_with("post-checkout\nold\nnew\n1\n"), "{log}");
            assert!(log.contains("inherit:old:new:1"), "{log}");
            assert_eq!(chain_command(&chained, &command).unwrap(), chained);
        }
        let command = chain_command("exit 0", "exit 23").unwrap();
        assert_eq!(
            Command::new("sh")
                .args(["-c", &command])
                .status()
                .unwrap()
                .code(),
            Some(23)
        );
    }

    #[test]
    fn javascript_preserves_dynamic_source_and_updates_idempotently() {
        for (file, source) in [
            (
                "config.cjs",
                "const hook = process.env.HOOK;\nmodule.exports = { 'post-checkout': hook, 'pre-commit': 'echo business' };\n",
            ),
            (
                "config.mjs",
                "const hook = process.env.HOOK;\nexport default { 'post-checkout': hook };\n",
            ),
            (
                "config.js",
                "const config = { 'post-checkout': process.env.HOOK };\nexport { config as default };\n",
            ),
        ] {
            let path = Path::new(file);
            let first =
                update_javascript(source, path, "jt codex inherit --post-checkout").unwrap();
            let repeated =
                update_javascript(&first, path, "jt codex inherit --post-checkout").unwrap();
            assert_eq!(first, repeated, "{file}");
            assert!(first.contains("process.env.HOOK"));
            assert_eq!(first.matches(JS_BEGIN).count(), 1);
            assert!(first.contains("\\\"$@\\\""));
            let with_suffix = format!("{first}// custom later addition\n");
            let updated =
                update_javascript(&with_suffix, path, "jt codex inherit --post-checkout").unwrap();
            assert!(updated.contains("// custom later addition\n"));
            assert!(!updated.contains("additionfunction"));
        }
    }

    #[test]
    fn lefthook_updates_named_task_without_changing_other_commands_or_jobs() {
        let run = "jt codex inherit --post-checkout \"{1}\" \"{2}\" \"{3}\"";
        for source in [
            "pre-commit:\n  commands:\n    lint:\n      run: echo lint\n",
            "post-checkout:\n  jobs:\n    - name: business\n      run: echo business\n",
            "post-checkout:\n  commands:\n    business:\n      run: echo business\n",
            "post-checkout:\n  commands:\n    jt-worktree-inherit:\n      run: obsolete\n    business:\n      run: echo business\n",
            "post-checkout:\n  commands: {}\n",
            "post-checkout: # retain post comment\n  commands: {} # retain commands comment\n",
        ] {
            let first = update_yaml(source, run).unwrap();
            assert_eq!(first, update_yaml(&first, run).unwrap());
            assert_eq!(first.matches("jt-worktree-inherit:").count(), 1);
            if source.contains("echo business") {
                assert!(first.contains("echo business"));
            }
            if source.contains("# retain") {
                assert!(first.contains("# retain post comment"));
                assert!(first.contains("# retain commands comment"));
            }
            assert!(!first.contains("obsolete"));
        }
        let toml = "[post-checkout.commands.business]\nrun = \"echo business\"\n";
        let first = update_toml(toml, run).unwrap();
        assert_eq!(first, update_toml(&first, run).unwrap());
        assert!(first.contains(toml));
        let jsonc = "{\n// business comment\n\"post-checkout\": {\"jobs\": [{\"run\": \"echo business\"}]}\n}";
        let value = json!({"run":run});
        let first = json_set(jsonc, &["post-checkout", "commands", TASK], &value).unwrap();
        assert_eq!(
            first,
            json_set(&first, &["post-checkout", "commands", TASK], &value).unwrap()
        );
        assert!(first.contains("// business comment"));
        assert!(first.contains("\"jobs\": [{\"run\": \"echo business\"}]"));
    }

    #[test]
    fn simple_config_priority_follows_manager_and_preserves_inactive_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(
            root.join(".simple-git-hooks.json"),
            "{\"pre-commit\":\"echo active\"}",
        )
        .unwrap();
        fs::write(
            root.join("simple-git-hooks.json"),
            "{\"post-checkout\":\"echo inactive\"}",
        )
        .unwrap();
        let package = json!({"simple-git-hooks":{"post-checkout":"echo package"}});
        fs::write(root.join("package.json"), package.to_string()).unwrap();
        let inactive = fs::read(root.join("simple-git-hooks.json")).unwrap();
        assert_eq!(
            update_simple(root, &package, "jt codex inherit --post-checkout").unwrap(),
            root.join(".simple-git-hooks.json")
        );
        assert_eq!(
            fs::read(root.join("simple-git-hooks.json")).unwrap(),
            inactive
        );
        assert_eq!(
            fs::read_to_string(root.join("package.json")).unwrap(),
            package.to_string()
        );
        let active: Value =
            serde_json::from_slice(&fs::read(root.join(".simple-git-hooks.json")).unwrap())
                .unwrap();
        assert_eq!(active["pre-commit"], "echo active");
        assert_eq!(active["preserveUnused"], true);
        assert!(script_mentions(
            &json!({"scripts":{"prepare":"lefthook install;simple-git-hooks"}}),
            "simple-git-hooks"
        ));
        assert_eq!(
            simple_custom_config(&json!({"scripts":{"prepare":"simple-git-hooks && echo ready"}}))
                .unwrap(),
            None
        );
    }

    #[test]
    fn config_paths_cannot_escape_or_follow_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(
            ensure_regular(temporary.path(), &temporary.path().join("../outside.json")).is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/tmp", temporary.path().join("linked")).unwrap();
            assert!(
                ensure_regular(
                    temporary.path(),
                    &temporary.path().join("linked/hooks.json")
                )
                .is_err()
            );
        }
    }
}
