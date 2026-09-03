use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTarget {
    pub root: PathBuf,
    pub branch: String,
    pub commit: String,
}

pub struct PreparedRepository {
    pub target: RepositoryTarget,
    pub origin: String,
    _lock: RepositoryLock,
}

struct RepositoryLock {
    path: PathBuf,
    _file: File,
}

impl RepositoryLock {
    fn acquire(repository: &Path) -> Result<Self> {
        let parent = repository.parent().with_context(|| {
            format!("backend repository has no parent: {}", repository.display())
        })?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create backend repository parent {}", parent.display()))?;
        let name = repository
            .file_name()
            .context("backend repository path has no directory name")?
            .to_string_lossy();
        let path = parent.join(format!(".{name}.nlab-api.lock"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "backend repository already in use or lock unavailable: {}",
                    path.display()
                )
            })?;
        writeln!(file, "pid={}", std::process::id())?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for RepositoryLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn resolve_path(configured: &Path, repository: Option<&str>) -> Result<PathBuf> {
    if !configured.as_os_str().is_empty() {
        return absolute_path(configured);
    }
    managed_clone_path(repository.context("backend repository URL missing from config")?)
}

pub fn managed_clone_path(repository: &str) -> Result<PathBuf> {
    let home = PathBuf::from(
        env::var_os("HOME").context("HOME is required to locate managed backend clones")?,
    );
    if !home.is_absolute() || home.parent().is_none() {
        bail!("HOME must be an absolute non-root path");
    }
    Ok(home
        .join(".local/share/nlab-api/repos")
        .join(managed_clone_name(repository)?))
}

pub fn prepare(
    repository_path: &Path,
    expected_origin: Option<&str>,
    requested_branch: Option<&str>,
    deadline: Instant,
) -> Result<PreparedRepository> {
    let repository_path = absolute_path(repository_path)?;
    let lock_target = if repository_path.exists() {
        repository_path
            .canonicalize()
            .with_context(|| format!("resolve backend repository {}", repository_path.display()))?
    } else {
        repository_path.clone()
    };
    let lock = RepositoryLock::acquire(&lock_target)?;
    if !repository_path.exists() {
        let origin = expected_origin.context("cannot clone backend without repository URL")?;
        clone_repository(origin, &repository_path, requested_branch, deadline)?;
    }
    let root = repository_path
        .canonicalize()
        .with_context(|| format!("resolve backend repository {}", repository_path.display()))?;
    if !root.join(".git").exists() {
        bail!("not a Git repository: {}", root.display());
    }
    let origin = origin_url(&root)?;
    if let Some(expected) = expected_origin {
        if !same_repository(expected, &origin) {
            bail!("backend origin mismatch: expected {expected}, found {origin}");
        }
    }
    ensure_clean(&root)?;
    let branch = requested_branch
        .map(str::to_owned)
        .unwrap_or(current_branch(&root)?);
    validate_branch(&root, &branch)?;
    switch_branch(&root, &branch, deadline)?;
    pull_ff_only(&root, &branch, deadline)?;
    let target = inspect(&root, &branch)?;
    Ok(PreparedRepository {
        target,
        origin,
        _lock: lock,
    })
}

pub fn current_branch(repo: &Path) -> Result<String> {
    let root = repo
        .canonicalize()
        .with_context(|| format!("resolve backend repository {}", repo.display()))?;
    let branch = git_text(&root, ["branch", "--show-current"])?;
    if branch.is_empty() {
        bail!(
            "backend repository is in detached HEAD state; pass --branch: {}",
            root.display()
        );
    }
    Ok(branch)
}

pub fn origin_url(repo: &Path) -> Result<String> {
    git_text(repo, ["remote", "get-url", "origin"])
        .with_context(|| format!("read backend origin in {}", repo.display()))
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
    ensure_clean(&target.root)?;
    let current = inspect(&target.root, &target.branch)?;
    if current.commit != target.commit {
        bail!(
            "backend branch moved during CodeGraph indexing: {} -> {}",
            target.commit,
            current.commit
        );
    }
    Ok(())
}

pub fn verify_unchanged(target: &RepositoryTarget) -> Result<()> {
    ensure_clean(&target.root)?;
    let current = inspect(&target.root, &target.branch)?;
    if current.commit != target.commit {
        bail!(
            "backend branch moved during generation: {} -> {}",
            target.commit,
            current.commit
        );
    }
    Ok(())
}

fn clone_repository(
    repository: &str,
    destination: &Path,
    branch: Option<&str>,
    deadline: Instant,
) -> Result<()> {
    if repository.trim().is_empty() || repository.starts_with('-') {
        bail!("backend repository URL must be non-empty and must not start with '-'");
    }
    let parent = destination
        .parent()
        .with_context(|| format!("clone destination has no parent: {}", destination.display()))?;
    let stage = tempfile::Builder::new()
        .prefix(".nlab-api-clone-")
        .tempdir_in(parent)
        .with_context(|| format!("create clone staging directory in {}", parent.display()))?;
    let checkout = stage.path().join("repository");
    let mut command = Command::new("git");
    command.arg("clone");
    if let Some(branch) = branch {
        command.args(["--branch", branch]);
    }
    command
        .arg("--")
        .arg(repository)
        .arg(&checkout)
        .current_dir(parent)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let status = run_until(&mut command, deadline)
        .with_context(|| format!("clone backend repository {repository}"))?;
    if !status.success() {
        bail!("git clone failed with status {status}");
    }
    fs::rename(&checkout, destination)
        .with_context(|| format!("promote backend clone to {}", destination.display()))
}

fn ensure_clean(root: &Path) -> Result<()> {
    let output = git_text(root, ["status", "--porcelain=v1", "--untracked-files=no"])?;
    if let Some(change) = output.lines().next() {
        bail!("backend repository has tracked changes: {change}");
    }
    Ok(())
}

fn validate_branch(root: &Path, branch: &str) -> Result<()> {
    if branch.trim().is_empty() || branch.starts_with('-') {
        bail!("backend branch must be non-empty and must not start with '-'");
    }
    git_bytes(root, ["check-ref-format", "--branch", branch])?;
    Ok(())
}

fn switch_branch(root: &Path, branch: &str, deadline: Instant) -> Result<()> {
    if git_text(root, ["branch", "--show-current"])? == branch {
        return Ok(());
    }
    let local_ref = format!("refs/heads/{branch}");
    if ref_exists(root, &local_ref)? {
        return run_git(root, ["switch", branch], "switch backend branch", deadline);
    }

    run_git(
        root,
        ["fetch", "--no-tags", "origin"],
        "fetch backend branches",
        deadline,
    )?;
    let remote = format!("origin/{branch}");
    let remote_ref = format!("refs/remotes/{remote}");
    if !ref_exists(root, &remote_ref)? {
        bail!("backend branch not found on origin: {branch}");
    }
    run_git(
        root,
        ["switch", "--track", "-c", branch, &remote],
        "create backend tracking branch",
        deadline,
    )
}

fn pull_ff_only(root: &Path, branch: &str, deadline: Instant) -> Result<()> {
    run_git(
        root,
        ["pull", "--ff-only", "origin", branch],
        "update backend repository",
        deadline,
    )
}

fn run_git<I, S>(root: &Path, arguments: I, action: &str, deadline: Instant) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command
        .args(arguments)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let status = run_until(&mut command, deadline).with_context(|| action.to_owned())?;
    if !status.success() {
        bail!("{action} failed with status {status}");
    }
    Ok(())
}

fn ref_exists(root: &Path, reference: &str) -> Result<bool> {
    let status = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", reference])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("inspect Git reference")?;
    Ok(status.success())
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

pub(crate) fn repository_name(repository: &str) -> Result<&str> {
    let repository = repository.trim().trim_end_matches('/');
    let tail = repository.rsplit(['/', ':']).next().unwrap_or_default();
    let name = tail.strip_suffix(".git").unwrap_or(tail);
    if name.is_empty() || matches!(name, "." | "..") {
        bail!("cannot derive backend repository name from {repository}");
    }
    Ok(name)
}

fn managed_clone_name(repository: &str) -> Result<String> {
    let name = repository_name(repository)?;
    let digest = Sha256::digest(normalize_repository(repository).as_bytes());
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{name}-{suffix}"))
}

fn same_repository(expected: &str, actual: &str) -> bool {
    normalize_repository(expected) == normalize_repository(actual)
}

fn normalize_repository(repository: &str) -> &str {
    let repository = repository.trim().trim_end_matches('/');
    repository.strip_suffix(".git").unwrap_or(repository)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests {
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

    fn repositories() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
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
        fs::write(upstream.join("contract.java"), "main").unwrap();
        git(&upstream, &["add", "contract.java"]);
        git(&upstream, &["commit", "-m", "initial"]);
        git(
            &upstream,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&upstream, &["push", "-u", "origin", "main"]);
        git(&upstream, &["switch", "-c", "feature"]);
        fs::write(upstream.join("contract.java"), "feature").unwrap();
        git(&upstream, &["commit", "-am", "feature"]);
        git(&upstream, &["push", "-u", "origin", "feature"]);
        git(&upstream, &["switch", "main"]);
        (root, remote, upstream, backend)
    }

    #[test]
    fn prepare_clones_and_switches_main_feature_main() {
        let (_root, remote, _upstream, backend) = repositories();
        let deadline = || Instant::now() + Duration::from_secs(10);

        {
            let prepared = prepare(
                &backend,
                Some(remote.to_str().unwrap()),
                Some("main"),
                deadline(),
            )
            .unwrap();
            assert_eq!(prepared.target.branch, "main");
        }
        fs::write(backend.join("local-note.txt"), "keep").unwrap();
        {
            let prepared = prepare(
                &backend,
                Some(remote.to_str().unwrap()),
                Some("feature"),
                deadline(),
            )
            .unwrap();
            assert_eq!(prepared.target.branch, "feature");
            assert_eq!(
                fs::read_to_string(backend.join("contract.java")).unwrap(),
                "feature"
            );
            assert_eq!(
                fs::read_to_string(backend.join("local-note.txt")).unwrap(),
                "keep"
            );
        }
        let prepared = prepare(
            &backend,
            Some(remote.to_str().unwrap()),
            Some("main"),
            deadline(),
        )
        .unwrap();
        assert_eq!(prepared.target.branch, "main");
        assert_eq!(
            fs::read_to_string(backend.join("contract.java")).unwrap(),
            "main"
        );
    }

    #[test]
    fn prepare_rejects_dirty_repository_and_wrong_origin() {
        let (root, remote, _upstream, backend) = repositories();
        git(
            root.path(),
            &["clone", remote.to_str().unwrap(), backend.to_str().unwrap()],
        );
        fs::write(backend.join("contract.java"), "dirty").unwrap();
        let error = prepare(
            &backend,
            Some(remote.to_str().unwrap()),
            Some("feature"),
            Instant::now() + Duration::from_secs(10),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("tracked changes"), "{error}");
        assert_eq!(current_branch(&backend).unwrap(), "main");

        git(&backend, &["checkout", "--", "contract.java"]);
        let other = root.path().join("other.git");
        git(
            root.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                other.to_str().unwrap(),
            ],
        );
        let error = prepare(
            &backend,
            Some(other.to_str().unwrap()),
            Some("main"),
            Instant::now() + Duration::from_secs(10),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("origin mismatch"), "{error}");
    }

    #[test]
    fn prepare_updates_and_ignores_untracked_codegraph() {
        let (root, remote, upstream, backend) = repositories();
        git(
            root.path(),
            &["clone", remote.to_str().unwrap(), backend.to_str().unwrap()],
        );
        fs::create_dir(backend.join(".codegraph")).unwrap();
        fs::write(backend.join(".codegraph/codegraph.db"), "index").unwrap();
        fs::write(upstream.join("contract.java"), "updated").unwrap();
        git(&upstream, &["add", "contract.java"]);
        git(&upstream, &["commit", "-m", "update"]);
        git(&upstream, &["push", "origin", "main"]);

        let prepared = prepare(
            &backend,
            Some(remote.to_str().unwrap()),
            Some("main"),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(backend.join("contract.java")).unwrap(),
            "updated"
        );
        assert!(prepared.target.commit.len() >= 7);
        assert!(backend.join(".codegraph/codegraph.db").is_file());
    }

    #[test]
    fn repository_lock_serializes_shared_clone() {
        let root = tempfile::tempdir().unwrap();
        let repository = root.path().join("backend");
        let first = RepositoryLock::acquire(&repository).unwrap();
        let error = RepositoryLock::acquire(&repository)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("already in use"), "{error}");
        drop(first);
        RepositoryLock::acquire(&repository).unwrap();
    }

    #[test]
    fn managed_clone_name_supports_ssh_and_https_urls() {
        assert_eq!(
            repository_name("git@example.com:team/backend.git").unwrap(),
            "backend"
        );
        assert_eq!(
            repository_name("https://example.com/team/backend").unwrap(),
            "backend"
        );
        assert_eq!(
            managed_clone_name("git@example.com:team/backend.git").unwrap(),
            managed_clone_name("git@example.com:team/backend").unwrap()
        );
        assert_ne!(
            managed_clone_name("git@example.com:team/backend.git").unwrap(),
            managed_clone_name("git@example.com:other/backend.git").unwrap()
        );
    }
}
