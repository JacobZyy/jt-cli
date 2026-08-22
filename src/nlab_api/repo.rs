use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTarget {
    pub root: PathBuf,
    pub branch: String,
    pub commit: String,
}

pub fn current_branch(repo: &Path) -> Result<String> {
    let root = repo
        .canonicalize()
        .with_context(|| format!("resolve backend repository {}", repo.display()))?;
    let branch = git_text(&root, ["branch", "--show-current"])?;
    if branch.is_empty() {
        bail!(
            "backend repository is in detached HEAD state: {}",
            root.display()
        );
    }
    Ok(branch)
}

pub fn inspect(repo: &Path, requested_branch: &str) -> Result<RepositoryTarget> {
    let root = repo
        .canonicalize()
        .with_context(|| format!("resolve backend repository {}", repo.display()))?;
    if !root.join(".git").exists() {
        bail!("not a Git repository: {}", root.display());
    }
    let branch = git_text(&root, ["branch", "--show-current"])?;
    if branch.is_empty() {
        bail!(
            "backend repository is in detached HEAD state: {}",
            root.display()
        );
    }
    if branch != requested_branch {
        bail!("backend branch mismatch: expected {requested_branch}, found {branch}");
    }
    let commit = git_text(&root, ["rev-parse", "HEAD"])?;
    let requested_commit = git_text(
        &root,
        ["rev-parse", &format!("{requested_branch}^{{commit}}")],
    )?;
    if commit != requested_commit {
        bail!("backend branch HEAD mismatch: {commit} != {requested_commit}");
    }

    Ok(RepositoryTarget {
        root,
        branch,
        commit,
    })
}

pub fn update_ff_only(target: &RepositoryTarget, deadline: Instant) -> Result<()> {
    let mut command = Command::new("git");
    command
        .args(["pull", "--ff-only", "origin", &target.branch])
        .current_dir(&target.root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let status = run_until(&mut command, deadline)
        .with_context(|| format!("update backend repository {}", target.root.display()))?;
    if !status.success() {
        bail!("git pull --ff-only failed with status {status}");
    }
    Ok(())
}

pub fn sync_codegraph(target: &RepositoryTarget, deadline: Instant) -> Result<()> {
    let action = if target.root.join(".codegraph/codegraph.db").is_file() {
        "sync"
    } else {
        "init"
    };
    let mut command = Command::new("codegraph");
    command
        .arg(action)
        .arg(&target.root)
        .current_dir(&target.root)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = run_until(&mut command, deadline)
        .with_context(|| format!("codegraph {action} {}", target.root.display()))?;
    if !status.success() {
        bail!("codegraph {action} failed with status {status}");
    }
    let current = git_text(&target.root, ["rev-parse", "HEAD"])?;
    if current != target.commit {
        bail!(
            "backend branch moved during CodeGraph indexing: {} -> {current}",
            target.commit
        );
    }
    Ok(())
}

pub fn verify_unchanged(target: &RepositoryTarget) -> Result<()> {
    let current = git_text(&target.root, ["rev-parse", "HEAD"])?;
    if current != target.commit {
        bail!(
            "backend branch moved during generation: {} -> {current}",
            target.commit
        );
    }
    Ok(())
}

fn run_until(command: &mut Command, deadline: Instant) -> Result<ExitStatus> {
    if Instant::now() >= deadline {
        bail!("generation deadline reached before starting command");
    }
    let mut child = command.spawn().context("start command")?;
    loop {
        if let Some(status) = child.try_wait().context("poll command")? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            child.kill().context("stop timed out command")?;
            let _ = child.wait();
            bail!("generation deadline reached during command");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn git_text<const N: usize>(root: &Path, arguments: [&str; N]) -> Result<String> {
    String::from_utf8(git_bytes(root, arguments)?)
        .context("decode Git output")
        .map(|value| value.trim().to_owned())
}

fn git_bytes<I, S>(root: &Path, arguments: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .context("start Git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Git returned non-zero");
        bail!("Git failed with status {}: {detail}", output.status);
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn git(root: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {}", arguments.join(" "));
    }

    #[test]
    fn update_ff_only_ignores_untracked_codegraph() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote.git");
        let upstream = root.path().join("upstream");
        let backend = root.path().join("backend");
        fs::create_dir(&upstream).unwrap();
        git(
            root.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
        );
        git(&upstream, &["init", "--initial-branch=main"]);
        git(&upstream, &["config", "user.name", "test"]);
        git(&upstream, &["config", "user.email", "test@example.com"]);
        fs::write(upstream.join("contract.java"), "one").unwrap();
        git(&upstream, &["add", "contract.java"]);
        git(&upstream, &["commit", "-m", "initial"]);
        git(
            &upstream,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&upstream, &["push", "-u", "origin", "main"]);
        git(
            root.path(),
            &["clone", remote.to_str().unwrap(), backend.to_str().unwrap()],
        );
        fs::create_dir(backend.join(".codegraph")).unwrap();
        fs::write(backend.join(".codegraph/codegraph.db"), "index").unwrap();

        let before = inspect(&backend, "main").unwrap();
        fs::write(upstream.join("contract.java"), "two").unwrap();
        git(&upstream, &["add", "contract.java"]);
        git(&upstream, &["commit", "-m", "update"]);
        git(&upstream, &["push", "origin", "main"]);

        update_ff_only(&before, Instant::now() + Duration::from_secs(10)).unwrap();
        let after = inspect(&backend, "main").unwrap();
        assert_ne!(before.commit, after.commit);
        assert!(backend.join(".codegraph/codegraph.db").is_file());
    }
}
