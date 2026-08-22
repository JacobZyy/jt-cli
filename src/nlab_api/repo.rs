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
