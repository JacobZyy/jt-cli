use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::node::error::{AppError, Result};
use crate::node::fs::{atomic_write, atomic_write_with_permissions, read_optional};
use crate::node::platform::first_executable;

const TEMPLATE_URL: &str =
    "https://raw.githubusercontent.com/JacobZyy/jt-cli/main/templates/zed/settings.json";
const TEMPLATE_PATH: &str = ".zed/settings.json";
const MAX_TEMPLATE_SIZE: usize = 1024 * 1024;
const MAX_TEMPLATE_SIZE_ARG: &str = "1048576";
const CURL_ARGS: [&str; 13] = [
    "-q",
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
    "--max-filesize",
    MAX_TEMPLATE_SIZE_ARG,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstallStatus {
    Created,
    Updated,
    Unchanged,
}

pub fn run() -> u8 {
    match execute() {
        Ok((InstallStatus::Created, path)) => {
            println!("created {}", path.display());
            0
        }
        Ok((InstallStatus::Updated, path)) => {
            println!("updated {}", path.display());
            0
        }
        Ok((InstallStatus::Unchanged, path)) => {
            println!("already configured {}", path.display());
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn execute() -> Result<(InstallStatus, PathBuf)> {
    let current =
        env::current_dir().map_err(|error| AppError::io("read current directory", None, error))?;
    let root = repository_root(&current)?;
    let template = fetch_template()?;
    validate_template(&template)?;
    install(&root, &template)
}

fn repository_root(directory: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| AppError::io("find Git repository root", None, error))?;
    if !output.status.success() {
        return Err(AppError::Invalid(
            "current directory must be inside a Git repository".to_owned(),
        ));
    }
    let root = std::str::from_utf8(&output.stdout)
        .map_err(|error| AppError::Decode {
            action: "decode Git repository root".to_owned(),
            detail: error.to_string(),
        })?
        .trim();
    if root.is_empty() {
        return Err(AppError::Invalid(
            "git returned no repository root".to_owned(),
        ));
    }
    fs::canonicalize(root)
        .map_err(|error| AppError::io("resolve Git repository root", Some(root.into()), error))
}

fn fetch_template() -> Result<Vec<u8>> {
    let environment = env::vars_os().collect::<BTreeMap<OsString, OsString>>();
    let curl = first_executable("curl", &environment)
        .ok_or_else(|| AppError::Invalid("curl is required to download Zed config".to_owned()))?;
    let output = Command::new(&curl)
        .args(CURL_ARGS)
        .arg(TEMPLATE_URL)
        .output()
        .map_err(|error| AppError::io("download Zed config template", Some(curl), error))?;
    if !output.status.success() {
        return Err(AppError::Command {
            action: "download Zed config template".to_owned(),
            status: output.status.code().unwrap_or(1),
            detail: last_nonempty_line(&output.stderr),
        });
    }
    if output.stdout.len() > MAX_TEMPLATE_SIZE {
        return Err(AppError::Invalid(format!(
            "Zed config template exceeds {MAX_TEMPLATE_SIZE} bytes"
        )));
    }
    Ok(output.stdout)
}

fn validate_template(content: &[u8]) -> Result<()> {
    match serde_json::from_slice::<Value>(content) {
        Ok(Value::Object(_)) => Ok(()),
        Ok(_) => Err(AppError::Invalid(
            "Zed config template must be a JSON object".to_owned(),
        )),
        Err(error) => Err(AppError::Invalid(format!(
            "Zed config template is invalid JSON: {error}"
        ))),
    }
}

fn install(root: &Path, content: &[u8]) -> Result<(InstallStatus, PathBuf)> {
    let path = root.join(TEMPLATE_PATH);
    reject_symlink(&root.join(".zed"))?;
    reject_symlink(&path)?;

    let current = read_optional(&path)?;
    if current.as_deref() == Some(content) {
        return Ok((InstallStatus::Unchanged, path));
    }
    if let Some(current) = current.as_deref() {
        backup(root, &path, current)?;
    }
    atomic_write(root, &path, current.as_deref(), content)?;
    let status = if current.is_some() {
        InstallStatus::Updated
    } else {
        InstallStatus::Created
    };
    Ok((status, path))
}

fn reject_symlink(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(AppError::UnsafePath(format!(
            "refuse to write through symlink: {}",
            path.display()
        )));
    }
    Ok(())
}

fn backup(root: &Path, path: &Path, content: &[u8]) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::Invalid(format!("system clock before epoch: {error}")))?
        .as_millis();
    let name = path
        .file_name()
        .ok_or_else(|| AppError::Invalid(format!("file has no name: {}", path.display())))?
        .to_string_lossy();
    let backup = path.with_file_name(format!("{name}.bak.{timestamp}"));
    let permissions = fs::metadata(path)
        .map_err(|error| AppError::io("read Zed config permissions", Some(path.into()), error))?
        .permissions();
    atomic_write_with_permissions(root, &backup, None, content, Some(permissions))
}

fn last_nonempty_line(content: &[u8]) -> String {
    String::from_utf8_lossy(content)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("curl returned non-zero")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn committed_template_is_valid() {
        validate_template(include_bytes!("../templates/zed/settings.json")).unwrap();
    }

    #[test]
    fn curl_ignores_user_config_and_limits_download() {
        assert_eq!(CURL_ARGS[0], "-q");
        assert!(
            CURL_ARGS
                .windows(2)
                .any(|args| args == ["--max-filesize", MAX_TEMPLATE_SIZE_ARG])
        );
        assert_eq!(
            MAX_TEMPLATE_SIZE_ARG.parse::<usize>().unwrap(),
            MAX_TEMPLATE_SIZE
        );
    }

    #[test]
    fn finds_repository_root_from_nested_directory() {
        let repository = tempdir().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        let nested = repository.path().join("packages/demo");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            repository_root(&nested).unwrap(),
            fs::canonicalize(repository.path()).unwrap()
        );
    }

    #[test]
    fn creates_reuses_and_safely_updates_config() {
        let repository = tempdir().unwrap();
        let first = b"{\"languages\":{}}\n";
        let second = b"{\"languages\":{\"Vue.js\":{}}}\n";

        let (status, path) = install(repository.path(), first).unwrap();
        assert_eq!(status, InstallStatus::Created);
        assert_eq!(fs::read(&path).unwrap(), first);

        assert_eq!(
            install(repository.path(), first).unwrap().0,
            InstallStatus::Unchanged
        );
        #[cfg(unix)]
        fs::set_permissions(&path, {
            use std::os::unix::fs::PermissionsExt;
            fs::Permissions::from_mode(0o600)
        })
        .unwrap();
        assert_eq!(
            install(repository.path(), second).unwrap().0,
            InstallStatus::Updated
        );
        assert_eq!(fs::read(&path).unwrap(), second);

        let backups = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.bak.")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path()).unwrap(), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(backups[0].path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn rejects_non_json_template() {
        assert!(validate_template(b"<html>not found</html>").is_err());
        assert!(validate_template(b"[]").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_zed_directory() {
        use std::os::unix::fs::symlink;

        let repository = tempdir().unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), repository.path().join(".zed")).unwrap();

        assert!(install(repository.path(), b"{}\n").is_err());
        assert!(outside.path().read_dir().unwrap().next().is_none());
    }
}
