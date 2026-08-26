use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use clap::Args;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const REPOSITORY_URL: &str = "https://github.com/JacobZyy/jt-cli";
const TARGET: &str = "aarch64-apple-darwin";
const NO_UPDATE_ENV: &str = "NLAB_API_NO_UPDATE";
const MANIFEST_NAME: &str = "nlab-api-manifest.json";
const INSTALL_MARKER_NAME: &str = ".nlab-api-managed";
const INSTALL_MARKER: &str = "nlab-api installer v1\n";

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Check without installing
    #[arg(long)]
    check: bool,
    /// Reinstall when target equals current version
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Deserialize)]
struct ReleaseManifest {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    version: String,
    tag: String,
    targets: Vec<String>,
}

#[derive(Debug)]
struct Release {
    tag: String,
    version: Version,
}

pub fn run(args: UpdateArgs) -> u8 {
    match update(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}

pub fn auto_update(arguments: &[OsString]) -> Result<Option<ExitCode>> {
    if arguments.iter().any(|argument| argument == "--no-update")
        || std::env::var_os(NO_UPDATE_ENV).is_some()
    {
        return Ok(None);
    }
    if let Err(error) = supported_target() {
        eprintln!("warning: nlab-api update skipped: {error:#}");
        return Ok(None);
    }

    let current = current_version()?;
    let release = match fetch_ready_release() {
        Ok(release) => release,
        Err(error) => {
            eprintln!("warning: nlab-api update check failed: {error:#}");
            return Ok(None);
        }
    };
    if release.version <= current {
        return Ok(None);
    }

    println!("Updating nlab-api {} to {}", current, release.version);
    let Some(executable) = managed_install_target()? else {
        eprintln!(
            "warning: automatic nlab-api update skipped because current binary is not installer-managed"
        );
        return Ok(None);
    };
    install_release(&release, &executable)?;
    reexecute(arguments)
}

fn update(args: UpdateArgs) -> Result<()> {
    supported_target()?;
    let current = current_version()?;
    let release = fetch_ready_release()?;
    match release.version.cmp(&current) {
        std::cmp::Ordering::Greater => {}
        std::cmp::Ordering::Equal if !args.force => {
            println!("nlab-api is already up to date ({current})");
            return Ok(());
        }
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Less => {
            println!(
                "latest published nlab-api {} is older than installed {}",
                release.version, current
            );
            return Ok(());
        }
    }

    if args.check {
        println!("Update available: {current} -> {}", release.version);
        return Ok(());
    }

    let executable = managed_install_target()?.context(
        "current nlab-api is not installer-managed; rerun install-nlab-api.sh before self-update",
    )?;
    install_release(&release, &executable)?;
    println!("Updated nlab-api to {}", release.version);
    println!("Binary: {}", executable.display());
    Ok(())
}

fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).context("invalid embedded nlab-api version")
}

fn supported_target() -> Result<()> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Ok(());
    }
    bail!("nlab-api self-update supports macOS Apple Silicon only")
}

fn fetch_ready_release() -> Result<Release> {
    let temporary = tempdir().context("create update staging directory")?;
    let manifest_path = temporary.path().join(MANIFEST_NAME);
    download(
        &format!("{REPOSITORY_URL}/releases/latest/download/{MANIFEST_NAME}"),
        &manifest_path,
    )?;
    let source = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    parse_manifest(&source)
}

fn parse_manifest(source: &str) -> Result<Release> {
    let manifest = serde_json::from_str::<ReleaseManifest>(source)
        .context("decode nlab-api release manifest")?;
    if manifest.schema_version != 1 {
        bail!(
            "unsupported nlab-api release manifest schema {}",
            manifest.schema_version
        );
    }
    let version = Version::parse(&manifest.version)
        .with_context(|| format!("invalid nlab-api release version {}", manifest.version))?;
    if manifest.tag != format!("v{version}") {
        bail!(
            "nlab-api release tag does not match version: {}",
            manifest.tag
        );
    }
    if !manifest.targets.iter().any(|target| target == TARGET) {
        bail!("nlab-api release does not support target {TARGET}");
    }
    Ok(Release {
        tag: manifest.tag,
        version,
    })
}

fn managed_install_target() -> Result<Option<PathBuf>> {
    let executable = std::env::current_exe().context("resolve current nlab-api executable")?;
    let file_name = executable.file_name().and_then(|name| name.to_str());
    if file_name != Some("nlab-api") {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(&executable).with_context(|| {
        format!(
            "inspect current nlab-api executable {}",
            executable.display()
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!(
            "nlab-api self-update refuses a non-regular executable: {}",
            executable.display()
        );
    }

    let parent = executable
        .parent()
        .context("current nlab-api executable has no parent")?;
    let marker = parent.join(INSTALL_MARKER_NAME);
    if !has_install_marker(&marker)? {
        return Ok(None);
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let home = home
        .canonicalize()
        .with_context(|| format!("resolve HOME {}", home.display()))?;
    let parent = parent
        .canonicalize()
        .context("resolve nlab-api install directory")?;
    if !parent.starts_with(&home) {
        bail!(
            "nlab-api self-update supports only executables under HOME: {}",
            executable.display()
        );
    }
    Ok(Some(executable))
}

fn has_install_marker(marker: &Path) -> Result<bool> {
    let marker_metadata = match fs::symlink_metadata(marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", marker.display()));
        }
    };
    if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
        bail!(
            "nlab-api install marker is not a regular file: {}",
            marker.display()
        );
    }
    let marker_source =
        fs::read_to_string(marker).with_context(|| format!("read {}", marker.display()))?;
    if marker_source != INSTALL_MARKER {
        bail!("invalid nlab-api install marker: {}", marker.display());
    }
    Ok(true)
}

fn install_release(release: &Release, executable: &Path) -> Result<()> {
    let temporary = tempdir().context("create nlab-api update staging directory")?;
    let archive_name = format!("nlab-api-{TARGET}.tar.gz");
    let archive = temporary.path().join(&archive_name);
    let checksum = temporary.path().join(format!("{archive_name}.sha256"));
    let root = format!("{REPOSITORY_URL}/releases/download/{}", release.tag);
    download(&format!("{root}/{archive_name}"), &archive)?;
    download(&format!("{root}/{archive_name}.sha256"), &checksum)?;
    verify_checksum(&archive, &checksum)?;

    let extract = temporary.path().join("extract");
    fs::create_dir(&extract).context("create nlab-api extraction directory")?;
    verify_archive(&archive)?;
    run_command(
        Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&extract),
        "extract nlab-api release",
    )?;
    let staged = extract.join("nlab-api");
    verify_binary(&staged, &release.version, "verify staged nlab-api")?;
    replace_binary(executable, &staged, &release.version)
}

fn download(url: &str, target: &Path) -> Result<()> {
    run_command(
        Command::new("curl")
            .args([
                "-fsSL",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "--retry",
                "3",
                "--max-time",
                "30",
                "-o",
            ])
            .arg(target)
            .arg(url),
        "download nlab-api release",
    )
}

fn verify_checksum(archive: &Path, checksum: &Path) -> Result<()> {
    let checksum_source = fs::read_to_string(checksum)
        .with_context(|| format!("read checksum {}", checksum.display()))?;
    let expected = checksum_source
        .split_whitespace()
        .next()
        .context("checksum file is empty")?;
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid SHA-256 checksum in {}", checksum.display());
    }
    let content = fs::read(archive).with_context(|| format!("read {}", archive.display()))?;
    let actual = Sha256::digest(content);
    let actual = actual
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("nlab-api release checksum mismatch");
    }
    Ok(())
}

fn verify_archive(archive: &Path) -> Result<()> {
    let output = command_output(
        Command::new("tar").args(["-tzf"]).arg(archive),
        "inspect nlab-api release archive",
    )?;
    let entries = String::from_utf8_lossy(&output);
    let entries = entries
        .lines()
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    if entries != ["nlab-api"] {
        bail!("nlab-api release archive contains unexpected files");
    }
    Ok(())
}

fn verify_binary(executable: &Path, expected: &Version, action: &str) -> Result<()> {
    let output = Command::new(executable)
        .arg("--version")
        .output()
        .with_context(|| format!("{action}: execute {}", executable.display()))?;
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let expected = format!("nlab-api {expected}");
    if output.status.success() && actual == expected {
        return Ok(());
    }
    bail!("{action} failed: expected {expected:?}, got {actual:?}");
}

fn replace_binary(executable: &Path, staged: &Path, version: &Version) -> Result<()> {
    let previous = fs::read(executable)
        .with_context(|| format!("read current nlab-api {}", executable.display()))?;
    let parent = executable
        .parent()
        .context("current nlab-api executable has no parent")?;
    let temporary = parent.join(format!(".nlab-api.tmp-{}", std::process::id()));
    let replacement =
        fs::read(staged).with_context(|| format!("read staged nlab-api {}", staged.display()))?;
    write_executable(&temporary, &replacement)?;
    if let Err(error) = fs::rename(&temporary, executable) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("replace {}", executable.display()));
    }

    if let Err(verification) = verify_binary(executable, version, "verify installed nlab-api") {
        let rollback = parent.join(format!(".nlab-api.rollback-{}", std::process::id()));
        let result = write_executable(&rollback, &previous)
            .and_then(|_| fs::rename(&rollback, executable).context("restore previous nlab-api"));
        return match result {
            Ok(()) => Err(anyhow::anyhow!(
                "{verification}; restored previous nlab-api"
            )),
            Err(error) => Err(anyhow::anyhow!("{verification}; rollback failed: {error}")),
        };
    }
    Ok(())
}

fn write_executable(path: &Path, content: &[u8]) -> Result<()> {
    fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    let mut permissions = fs::metadata(path)
        .with_context(|| format!("inspect {}", path.display()))?
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(path, permissions)
        .with_context(|| format!("make {} executable", path.display()))
}

fn reexecute(arguments: &[OsString]) -> Result<Option<ExitCode>> {
    let executable = std::env::current_exe().context("resolve nlab-api executable for restart")?;
    let status = Command::new(&executable)
        .args(arguments)
        .env(NO_UPDATE_ENV, "1")
        .status()
        .with_context(|| format!("restart nlab-api {}", executable.display()))?;
    let code = status.code().unwrap_or(1).clamp(0, u8::MAX as i32) as u8;
    Ok(Some(ExitCode::from(code)))
}

fn run_command(command: &mut Command, action: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("{action}: start command"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!(
        "{action} failed with status {}; {}",
        output.status.code().unwrap_or(1),
        if detail.is_empty() {
            "no details"
        } else {
            &detail
        }
    );
}

fn command_output(command: &mut Command, action: &str) -> Result<Vec<u8>> {
    let output = command
        .output()
        .with_context(|| format!("{action}: start command"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!(
        "{action} failed with status {}; {}",
        output.status.code().unwrap_or(1),
        if detail.is_empty() {
            "no details"
        } else {
            &detail
        }
    );
}

#[cfg(test)]
mod tests {
    use super::{INSTALL_MARKER, TARGET, has_install_marker, parse_manifest, verify_checksum};
    use std::fs;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[test]
    fn parses_ready_manifest_for_current_target() {
        let release = parse_manifest(
            r#"{
                "schemaVersion": 1,
                "version": "1.11.0",
                "tag": "v1.11.0",
                "targets": ["aarch64-apple-darwin"]
            }"#,
        )
        .unwrap();
        assert_eq!(release.tag, "v1.11.0");
        assert_eq!(release.version.to_string(), "1.11.0");
    }

    #[test]
    fn rejects_manifest_without_current_target() {
        let error = parse_manifest(
            r#"{
                "schemaVersion": 1,
                "version": "1.11.0",
                "tag": "v1.11.0",
                "targets": ["x86_64-apple-darwin"]
            }"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains(TARGET));
    }

    #[test]
    fn verifies_archive_checksum() {
        let directory = tempdir().unwrap();
        let archive = directory.path().join("archive");
        let checksum = directory.path().join("archive.sha256");
        let content = b"nlab-api";
        fs::write(&archive, content).unwrap();
        let digest = Sha256::digest(content);
        let digest = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        fs::write(&checksum, format!("{digest}  archive\n")).unwrap();
        verify_checksum(&archive, &checksum).unwrap();
    }

    #[test]
    fn accepts_only_exact_installer_marker() {
        let directory = tempdir().unwrap();
        let marker = directory.path().join(".nlab-api-managed");
        assert!(!has_install_marker(&marker).unwrap());
        fs::write(&marker, INSTALL_MARKER).unwrap();
        assert!(has_install_marker(&marker).unwrap());
        fs::write(&marker, "unowned\n").unwrap();
        assert!(has_install_marker(&marker).is_err());
    }
}
