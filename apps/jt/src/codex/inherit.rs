use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const MANIFEST: &str = ".worktreeinclude";
const START: &str = "# BEGIN jt codex local configuration";
const END: &str = "# END jt codex local configuration";
pub(super) const SCRIPT: &str = include_str!("inherit.mjs");
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
    #[derive(serde::Deserialize)]
    struct Report {
        copied: usize,
    }

    let mut command = Command::new("node");
    command.args(["--input-type=module", "-", "--json"]);
    if let Some(checkout) = checkout {
        command.arg("--post-checkout").args(checkout);
    }
    let mut child = command
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("cannot run local configuration inheritance with Node: {error}")
        })?;
    let mut input = child.stdin.take().ok_or("missing Node script input")?;
    let written = input.write_all(SCRIPT.as_bytes());
    drop(input);
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    written.map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice::<Report>(&output.stdout)
        .map(|report| report.copied)
        .map_err(|error| format!("invalid local configuration inheritance report: {error}"))
}

#[cfg(test)]
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

#[cfg(test)]
#[path = "inherit_tests.rs"]
mod tests;
