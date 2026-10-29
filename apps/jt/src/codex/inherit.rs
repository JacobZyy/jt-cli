use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Component, Path};
use std::process::Command;

use ignore::WalkBuilder;
use ignore::gitignore::GitignoreBuilder;

const MANIFEST: &str = ".worktreeinclude";
const START: &str = "# BEGIN jt codex local configuration";
const END: &str = "# END jt codex local configuration";
const LEFTHOOK_LOCAL_STEMS: &[&str] = &[
    "lefthook-local",
    ".lefthook-local",
    ".config/lefthook-local",
];
const DEFAULTS: &[&str] = &[
    ".env.local",
    ".env.*.local",
    ".nlab/*.local.json",
    ".codex/config.toml",
    ".codex/hooks.json",
    ".claude/settings.local.json",
    "CLAUDE.local.md",
    "AGENTS.override.md",
];

pub fn update_manifest(root: &Path, local_overrides: &[&str]) -> Result<(), String> {
    let path = root.join(MANIFEST);
    let current = read_manifest(&path)?.unwrap_or_default();
    let custom = current
        .split_inclusive('\n')
        .filter(|line| {
            let pattern = line.trim().trim_start_matches('/').trim_end_matches('/');
            ![".nlab", ".codex", ".claude"].iter().any(|directory| {
                pattern == *directory || pattern.strip_suffix("/**") == Some(*directory)
            })
        })
        .collect::<String>();
    let mut patterns = DEFAULTS.to_vec();
    for pattern in local_overrides {
        if !patterns.contains(pattern) {
            patterns.push(pattern);
        }
    }
    let block = format!("{START}\n{}\n{END}\n", patterns.join("\n"));
    let next = match (custom.find(START), custom.find(END)) {
        (Some(start), Some(end)) if end > start => {
            let end = end + END.len();
            let suffix = custom[end..].strip_prefix('\n').unwrap_or(&custom[end..]);
            format!("{}{block}{suffix}", &custom[..start])
        }
        (None, None) => format!("{block}{custom}"),
        _ => return Err(format!("invalid JT managed block in {MANIFEST}")),
    };
    if next == current {
        return Ok(());
    }
    let mut file = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
    if let Ok(metadata) = fs::metadata(&path) {
        file.as_file()
            .set_permissions(metadata.permissions())
            .map_err(|error| error.to_string())?;
    }
    file.write_all(next.as_bytes())
        .map_err(|error| error.to_string())?;
    if read_manifest(&path)?.unwrap_or_default() != current {
        return Err(format!("{MANIFEST} concurrently changed"));
    }
    file.persist(&path).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn copy(root: &Path, checkout: Option<&[String]>) -> Result<usize, String> {
    if checkout.is_some_and(|args| {
        args.len() != 3
            || !matches!(args[0].len(), 40 | 64)
            || !args[0].bytes().all(|byte| byte == b'0')
            || args[2] != "1"
    }) {
        return Ok(0);
    }
    let root = match git(root, &["rev-parse", "--show-toplevel"]) {
        Ok(directory) => {
            fs::canonicalize(directory.trim_end()).map_err(|error| error.to_string())?
        }
        Err(error) if error.contains("not a git repository") => return Ok(0),
        Err(error) => return Err(error),
    };
    let git_dir = git(&root, &["rev-parse", "--path-format=absolute", "--git-dir"])?;
    let common_dir = git(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if git_dir.trim_end() == common_dir.trim_end() {
        return Ok(0);
    }
    let inventory = git(&root, &["worktree", "list", "--porcelain", "-z"])?;
    let source = inventory
        .split('\0')
        .next()
        .and_then(|line| line.strip_prefix("worktree "))
        .ok_or("cannot locate primary worktree")?;
    let source = fs::canonicalize(source)
        .map_err(|error| format!("primary worktree unavailable: {error}"))?;
    let manifest = match read_manifest(&root.join(MANIFEST))? {
        Some(manifest) => manifest,
        None => read_manifest(&source.join(MANIFEST))?
            .ok_or("missing .worktreeinclude; run jt codex init in the primary worktree")?,
    };
    let mut builder = GitignoreBuilder::new(&source);
    for line in manifest.lines() {
        let pattern = line.strip_prefix('!').unwrap_or(line);
        if Path::new(pattern)
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(format!("{MANIFEST} pattern escapes project: {line}"));
        }
        builder
            .add_line(None, line)
            .map_err(|error| format!("invalid {MANIFEST}: {error}"))?;
    }
    let matcher = builder.build().map_err(|error| error.to_string())?;
    let mut copied = 0;
    let mut walk = WalkBuilder::new(&source);
    walk.hidden(false)
        .parents(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .follow_links(false)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                ".git"
                    | ".codegraph"
                    | "node_modules"
                    | "target"
                    | "dist"
                    | "build"
                    | "cache"
                    | "logs"
                    | "reports"
            ) && ![
                ".lock",
                ".log",
                ".db",
                ".db-wal",
                ".db-shm",
                ".sqlite",
                ".sqlite3",
                ".sqlite-wal",
                ".sqlite-shm",
            ]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        });
    for entry in walk.build() {
        let entry = entry.map_err(|error| error.to_string())?;
        let relative = entry
            .path()
            .strip_prefix(&source)
            .map_err(|error| error.to_string())?;
        if entry.file_type().is_some_and(|kind| kind.is_dir())
            || !matcher
                .matched_path_or_any_parents(entry.path(), false)
                .is_ignore()
        {
            continue;
        }
        let destination = root.join(relative);
        if configuration_exists(&destination)? {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err(format!(
                "refusing non-file local configuration: {}",
                entry.path().display()
            ));
        }
        prepare_parents(&root, relative)?;
        if configuration_is_shadowed(&root, &source, relative)? {
            continue;
        }
        let mut output = tempfile::NamedTempFile::new_in(destination.parent().unwrap())
            .map_err(|error| error.to_string())?;
        let mut input = fs::File::open(entry.path()).map_err(|error| error.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
        output
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|error| error.to_string())?;
        match output.persist_noclobber(&destination) {
            Ok(_) => {}
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create {}: {error}", destination.display())),
        }
        copied += 1;
    }
    Ok(copied)
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

fn read_manifest(path: &Path) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => Err(format!("refusing non-file {}", path.display())),
        Ok(_) => fs::read_to_string(path)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn configuration_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn prepare_parents(root: &Path, relative: &Path) -> Result<(), String> {
    let mut directory = root.to_owned();
    for component in relative.parent().unwrap_or(Path::new("")).components() {
        let Component::Normal(name) = component else {
            return Err("local configuration path escapes project".to_owned());
        };
        directory.push(name);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(format!("refusing non-directory {}", directory.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                fs::create_dir(&directory).map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn configuration_is_shadowed(root: &Path, source: &Path, relative: &Path) -> Result<bool, String> {
    if relative
        .with_extension("")
        .to_str()
        .is_some_and(|stem| LEFTHOOK_LOCAL_STEMS.contains(&stem))
    {
        let mut formats = 0;
        for stem in LEFTHOOK_LOCAL_STEMS {
            for extension in ["yml", "yaml", "toml", "json", "jsonc"] {
                let alias = format!("{stem}.{extension}");
                if configuration_exists(&root.join(&alias))? {
                    return Ok(true);
                }
                if configuration_exists(&source.join(alias))? {
                    formats += 1;
                }
            }
        }
        if formats > 1 {
            return Err("multiple Lefthook local override formats or paths; select one in the primary worktree".to_owned());
        }
    }
    if relative == Path::new(".nlab/cli.local.json") {
        return Ok(
            configuration_exists(&source.join(".nlab/nlab-api.local.json"))?
                || configuration_exists(&root.join(".nlab/nlab-api.local.json"))?,
        );
    }
    if relative != Path::new(".nlab/nlab-api.local.json") {
        return Ok(false);
    }
    let Some(content) = read_manifest(&root.join(".nlab/cli.local.json"))? else {
        return Ok(false);
    };
    Ok(
        serde_json::from_str::<serde_json::Value>(&content).is_ok_and(|value| {
            value.as_object().is_some_and(|object| {
                object.len() == 2
                    && object.get("version").and_then(serde_json::Value::as_u64) == Some(1)
                    && matches!(
                        object.get("runner").and_then(serde_json::Value::as_str),
                        Some("nlab" | "jt" | "nlab-api")
                    )
            })
        }),
    )
}

#[cfg(test)]
#[path = "inherit_tests.rs"]
mod tests;
