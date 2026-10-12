use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize)]
struct Worktree {
    root: PathBuf,
    head: String,
    branch: Option<String>,
    primary: bool,
    locked: bool,
    prunable: bool,
    dirty: Option<bool>,
    integrated: Option<bool>,
}

// These lifecycle facts come from the caller's current Codex attachment/task
// inventory. Git cannot establish task completion, pinning, or ownership.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    version: u8,
    root: PathBuf,
    head: String,
    primary_head: String,
    attachment_identity_key: PathBuf,
    all_tasks_known: bool,
    managed: bool,
    pinned: bool,
    permanent: bool,
    shared: bool,
    tasks: Vec<Task>,
    validation: Validation,
    ignored_files_reviewed: bool,
    ignored_backups: Vec<IgnoredBackup>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    id: String,
    complete: bool,
    running: bool,
    retain: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    head: String,
    command: String,
    exit_code: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IgnoredBackup {
    source: PathBuf,
    backup: PathBuf,
}

pub fn run(check: Option<&Path>) -> u8 {
    match std::env::current_dir()
        .map_err(|error| error.to_string())
        .and_then(|root| report(&root, check))
    {
        Ok((report, passed)) => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            u8::from(!passed)
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn report(root: &Path, check: Option<&Path>) -> Result<(serde_json::Value, bool), String> {
    let worktrees = inventory(root)?;
    let Some(check) = check else {
        return Ok((
            json!({
                "operation": "inventory",
                "automatic_recycling": false,
                "worktrees": worktrees,
                "protection": "All worktrees remain protected until current Codex attachment/task lifecycle facts are supplied and native archive protection checks pass.",
                "next_step": "Use --check <evidence.json>, then Codex archive_worktree with the exact identityKey from list_artifacts. No directories are removed by this command.",
            }),
            true,
        ));
    };
    let evidence: Evidence = serde_json::from_slice(
        &fs::read(check).map_err(|error| format!("cannot read {}: {error}", check.display()))?,
    )
    .map_err(|error| format!("invalid archive evidence: {error}"))?;
    let primary = worktrees.first().ok_or("no primary worktree")?;
    let candidate = worktrees
        .iter()
        .find(|worktree| worktree.root == evidence.root)
        .ok_or(
            "evidence root is not a registered worktree; unknown directories remain protected",
        )?;
    let mut blockers = blockers(candidate, primary, &evidence);
    if let Err(error) = check_backups(candidate, &evidence.ignored_backups) {
        blockers.push(error);
    }
    let passed = blockers.is_empty();
    Ok((
        json!({
            "operation": "archive-check",
            "automatic_recycling": false,
            "git_and_backup_checks_passed": passed,
        "lifecycle_facts": "Task, ownership, validation, and ignored-file review facts are caller assertions; not independently verified from Codex state. Refresh task and attachment inventory immediately before native archive.",
            "blockers": blockers,
            "worktree": candidate,
            "native_archive_review": if passed { Some(json!({
                "tool": "archive_worktree",
                "arguments": { "root": evidence.attachment_identity_key },
                "requires_fresh_native_protection_checks": true,
            })) } else { None },
            "recovery": "Native archive saves uncommitted and unpushed work; necessary ignored files must remain in the verified external backups. No deletion or archive was performed.",
        }),
        passed,
    ))
}

fn blockers(candidate: &Worktree, primary: &Worktree, evidence: &Evidence) -> Vec<String> {
    let mut reasons = Vec::new();
    for (blocked, reason) in [
        (evidence.version != 1, "unsupported evidence version"),
        (candidate.primary, "primary worktree is protected"),
        (candidate.locked, "locked worktree is protected"),
        (
            candidate.prunable,
            "stale registration is protected; git worktree prune is not directory recovery",
        ),
        (
            primary.branch.is_none(),
            "primary worktree must be on a branch",
        ),
        (
            primary.dirty != Some(false),
            "primary worktree must be clean",
        ),
        (
            candidate.dirty != Some(false),
            "worktree has uncommitted changes or cannot be inspected",
        ),
        (
            candidate.integrated != Some(true),
            "worktree HEAD is not integrated into current primary HEAD",
        ),
        (
            candidate.head != evidence.head || primary.head != evidence.primary_head,
            "Git HEAD changed since lifecycle evidence was collected",
        ),
        (
            !evidence.root.is_absolute() || evidence.attachment_identity_key != evidence.root,
            "exact current Codex attachment identityKey is required",
        ),
        (
            !evidence.managed || !evidence.all_tasks_known || evidence.tasks.is_empty(),
            "unknown ownership or task inventory is protected",
        ),
        (
            evidence.pinned || evidence.permanent,
            "pinned or permanent worktree is protected",
        ),
        (
            evidence.shared || evidence.tasks.len() > 1,
            "shared worktree is protected",
        ),
        (
            evidence
                .tasks
                .iter()
                .any(|task| task.id.is_empty() || !task.complete || task.running || task.retain),
            "all tasks must be complete, stopped, and no longer retained",
        ),
        (
            evidence.validation.exit_code != 0
                || evidence.validation.command.trim().is_empty()
                || evidence.validation.head != primary.head,
            "successful validation of current integrated primary HEAD is required",
        ),
        (
            !evidence.ignored_files_reviewed,
            "necessary ignored files must be reviewed and backed up outside the worktree",
        ),
    ] {
        if blocked {
            reasons.push(reason.to_owned());
        }
    }
    reasons
}

fn check_backups(candidate: &Worktree, backups: &[IgnoredBackup]) -> Result<(), String> {
    let root = fs::canonicalize(&candidate.root)
        .map_err(|error| format!("cannot inspect worktree root: {error}"))?;
    for backup in backups {
        if backup.source.as_os_str().is_empty()
            || backup
                .source
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || !backup.backup.is_absolute()
        {
            return Err(
                "ignored backup requires a relative source file and absolute external backup file"
                    .to_owned(),
            );
        }
        let source = root.join(&backup.source);
        let destination = fs::canonicalize(&backup.backup).map_err(|error| {
            format!(
                "cannot inspect ignored backup {}: {error}",
                backup.backup.display()
            )
        })?;
        let source_root = fs::canonicalize(&source).map_err(|error| {
            format!(
                "cannot inspect ignored source {}: {error}",
                source.display()
            )
        })?;
        if !source_root.starts_with(&root)
            || destination.starts_with(&root)
            || !fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.is_file())
            || !destination.is_file()
        {
            return Err("ignored backups must preserve regular files outside the worktree; list necessary directory contents individually".to_owned());
        }
        if fs::read(&source).map_err(|error| error.to_string())?
            != fs::read(&destination).map_err(|error| error.to_string())?
        {
            return Err(format!(
                "ignored backup differs from source: {}",
                backup.source.display()
            ));
        }
    }
    Ok(())
}

fn inventory(root: &Path) -> Result<Vec<Worktree>, String> {
    let bytes = git(root, &["worktree", "list", "--porcelain", "-z"])?;
    let records = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let mut worktrees = Vec::new();
    for record in records.split("\0\0").filter(|record| !record.is_empty()) {
        let mut worktree = Worktree {
            root: PathBuf::new(),
            head: String::new(),
            branch: None,
            primary: worktrees.is_empty(),
            locked: false,
            prunable: false,
            dirty: None,
            integrated: None,
        };
        for field in record.split('\0') {
            if let Some(root) = field.strip_prefix("worktree ") {
                worktree.root = PathBuf::from(root);
            } else if let Some(head) = field.strip_prefix("HEAD ") {
                worktree.head = head.to_owned();
            } else if let Some(branch) = field.strip_prefix("branch ") {
                worktree.branch = Some(branch.to_owned());
            } else if field == "locked" || field.starts_with("locked ") {
                worktree.locked = true;
            } else if field == "prunable" || field.starts_with("prunable ") {
                worktree.prunable = true;
            }
        }
        if worktree.root.as_os_str().is_empty() {
            return Err("invalid git worktree inventory".to_owned());
        }
        worktree.dirty = git(
            &worktree.root,
            &["status", "--porcelain", "--untracked-files=normal"],
        )
        .ok()
        .map(|status| !status.is_empty());
        worktrees.push(worktree);
    }
    if let Some(head) = worktrees.first().map(|primary| primary.head.clone()) {
        for worktree in &mut worktrees {
            worktree.integrated = Command::new("git")
                .current_dir(root)
                .args(["merge-base", "--is-ancestor", &worktree.head, &head])
                .output()
                .ok()
                .and_then(|output| match output.status.code() {
                    Some(0) => Some(true),
                    Some(1) => Some(false),
                    _ => None,
                });
        }
    }
    Ok(worktrees)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> Evidence {
        serde_json::from_value(json!({
            "version": 1, "root": "/worktree", "head": "abc", "primary_head": "def",
            "attachment_identity_key": "/worktree", "all_tasks_known": true,
            "managed": true, "pinned": false, "permanent": false, "shared": false,
            "tasks": [{ "id": "task", "complete": true, "running": false, "retain": false }],
            "validation": { "head": "def", "command": "cargo test --locked", "exit_code": 0 },
            "ignored_files_reviewed": true, "ignored_backups": [],
        }))
        .unwrap()
    }

    fn worktree(primary: bool) -> Worktree {
        Worktree {
            root: PathBuf::from(if primary { "/primary" } else { "/worktree" }),
            head: if primary { "def" } else { "abc" }.to_owned(),
            branch: primary.then(|| "refs/heads/main".to_owned()),
            primary,
            locked: false,
            prunable: false,
            dirty: Some(false),
            integrated: Some(true),
        }
    }

    #[test]
    fn protects_primary_locked_dirty_and_unintegrated_worktrees() {
        let primary = worktree(true);
        assert!(blockers(&worktree(false), &primary, &evidence()).is_empty());
        assert!(!blockers(&primary, &primary, &evidence()).is_empty());
        for mutate in [
            |tree: &mut Worktree| tree.locked = true,
            |tree: &mut Worktree| tree.dirty = Some(true),
            |tree: &mut Worktree| tree.integrated = Some(false),
            |tree: &mut Worktree| tree.head = "changed".to_owned(),
        ] {
            let mut candidate = worktree(false);
            mutate(&mut candidate);
            assert!(!blockers(&candidate, &primary, &evidence()).is_empty());
        }
        let mut detached = worktree(true);
        detached.branch = None;
        assert!(!blockers(&worktree(false), &detached, &evidence()).is_empty());
    }

    #[test]
    fn requires_complete_stopped_unretained_tasks_known_ownership_and_validation() {
        for mutate in [
            |e: &mut Evidence| e.all_tasks_known = false,
            |e: &mut Evidence| e.managed = false,
            |e: &mut Evidence| e.attachment_identity_key = PathBuf::new(),
            |e: &mut Evidence| e.pinned = true,
            |e: &mut Evidence| e.permanent = true,
            |e: &mut Evidence| e.shared = true,
            |e: &mut Evidence| e.tasks[0].complete = false,
            |e: &mut Evidence| e.tasks[0].running = true,
            |e: &mut Evidence| e.tasks[0].retain = true,
            |e: &mut Evidence| e.validation.exit_code = 1,
            |e: &mut Evidence| e.validation.head = "stale".to_owned(),
            |e: &mut Evidence| e.ignored_files_reviewed = false,
            |e: &mut Evidence| e.tasks.clear(),
        ] {
            let mut facts = evidence();
            mutate(&mut facts);
            assert!(!blockers(&worktree(false), &worktree(true), &facts).is_empty());
        }
    }

    #[test]
    fn ignored_backups_must_match_and_survive_worktree_archive() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("worktree");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".env"), "necessary").unwrap();
        let mut candidate = worktree(false);
        candidate.root = root.clone();
        let mut backup = IgnoredBackup {
            source: PathBuf::from(".env"),
            backup: directory.path().join("saved.env"),
        };
        fs::write(&backup.backup, "necessary").unwrap();
        assert!(check_backups(&candidate, std::slice::from_ref(&backup)).is_ok());
        fs::write(&backup.backup, "stale").unwrap();
        assert!(check_backups(&candidate, std::slice::from_ref(&backup)).is_err());
        backup.backup = root.join(".env");
        assert!(check_backups(&candidate, std::slice::from_ref(&backup)).is_err());
        backup.source = PathBuf::from("../saved.env");
        assert!(check_backups(&candidate, &[backup]).is_err());
    }

    #[test]
    fn inventory_checks_real_ancestry_and_never_removes_worktrees() {
        let directory = tempfile::tempdir().unwrap();
        let primary = directory.path().join("primary");
        let candidate = directory.path().join("worktree with spaces");
        fs::create_dir(&primary).unwrap();
        git(&primary, &["init", "--initial-branch", "main"]).unwrap();
        git(&primary, &["config", "user.email", "test@example.com"]).unwrap();
        git(&primary, &["config", "user.name", "Test"]).unwrap();
        fs::write(primary.join("source"), "primary\n").unwrap();
        git(&primary, &["add", "source"]).unwrap();
        git(&primary, &["commit", "-m", "initial"]).unwrap();
        git(
            &primary,
            &["worktree", "add", "--detach", candidate.to_str().unwrap()],
        )
        .unwrap();
        let trees = inventory(&candidate).unwrap();
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[0].root, fs::canonicalize(&primary).unwrap());
        assert!(trees[0].primary);
        assert_eq!(trees[1].integrated, Some(true));
        assert_eq!(trees[1].dirty, Some(false));
        fs::write(candidate.join("source"), "task work\n").unwrap();
        git(&candidate, &["add", "source"]).unwrap();
        git(&candidate, &["commit", "-m", "task work"]).unwrap();
        assert_eq!(inventory(&primary).unwrap()[1].integrated, Some(false));
        git(&primary, &["worktree", "lock", candidate.to_str().unwrap()]).unwrap();
        assert!(inventory(&primary).unwrap()[1].locked);
        let (report, passed) = report(&primary, None).unwrap();
        assert!(passed);
        assert_eq!(report["automatic_recycling"], false);
        assert!(report["protection"].as_str().unwrap().contains("protected"));
        assert!(candidate.is_dir());
    }
}
