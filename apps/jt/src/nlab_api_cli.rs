use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};

const CONFIG_VERSION: u8 = 1;
const LOCAL_CONFIG_PATH: &str = ".nlab/cli.local.json";
const GITIGNORE_ENTRY: &str = "/.nlab/cli.local.json";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum ConfiguredRunner {
    Jt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Runner {
    Standalone,
    Jt,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Frontend project owning the local runner preference
    #[arg(long, value_name = "path", default_value = ".")]
    project: PathBuf,
    /// Use jt's embedded nlab-api implementation
    #[arg(
        long,
        value_enum,
        required_unless_present = "unset",
        conflicts_with = "unset"
    )]
    runner: Option<ConfiguredRunner>,
    /// Remove local preference and restore standalone nlab-api
    #[arg(long, required_unless_present = "runner", conflicts_with = "runner")]
    unset: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalConfig {
    version: u8,
    runner: ConfiguredRunner,
}

pub fn configure(args: ConfigArgs) -> u8 {
    match configure_inner(args) {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

pub fn forward_if_standalone(arguments: &[OsString]) -> Result<Option<u8>, String> {
    let Some(arguments) = standalone_arguments(arguments) else {
        return Ok(None);
    };
    if arguments
        .iter()
        .any(|argument| argument == "-h" || argument == "--help")
    {
        return Ok(None);
    }
    let Some(command) = arguments.first() else {
        return Ok(None);
    };
    if command != "init" && command != "generate" {
        return Ok(None);
    }
    let project = project_argument(&arguments[1..])?;
    match runner(&project)? {
        Runner::Standalone => execute_standalone(arguments).map(Some),
        Runner::Jt => Ok(None),
    }
}

fn runner(project: &Path) -> Result<Runner, String> {
    let project = resolve_project(project)?;
    let path = project.join(LOCAL_CONFIG_PATH);
    reject_symlink_path(&project, &path)?;
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Runner::Standalone);
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let config = serde_json::from_str::<LocalConfig>(&source)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    if config.version != CONFIG_VERSION {
        return Err(format!(
            "unsupported local nlab-api config version {}; expected {CONFIG_VERSION}",
            config.version
        ));
    }
    Ok(match config.runner {
        ConfiguredRunner::Jt => Runner::Jt,
    })
}

fn configure_inner(args: ConfigArgs) -> Result<String, String> {
    let project = resolve_project(&args.project)?;
    ensure_gitignore(&project)?;
    let path = project.join(LOCAL_CONFIG_PATH);
    reject_symlink_path(&project, &path)?;

    if args.unset {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot remove {}: {error}", path.display())),
        }
        return Ok(format!(
            "nlab-api runner reset to standalone for {}",
            project.display()
        ));
    }

    let config = LocalConfig {
        version: CONFIG_VERSION,
        runner: args.runner.expect("clap requires runner or unset"),
    };
    let mut source = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("cannot serialize local nlab-api config: {error}"))?;
    source.push(b'\n');
    atomic_write(&project, &path, &source)?;
    Ok(format!(
        "nlab-api runner configured as jt for {}",
        project.display()
    ))
}

fn resolve_project(project: &Path) -> Result<PathBuf, String> {
    let project = if project.is_absolute() {
        project.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot read current directory: {error}"))?
            .join(project)
    };
    let project = project.canonicalize().map_err(|error| {
        format!(
            "cannot resolve frontend project {}: {error}",
            project.display()
        )
    })?;
    if !project.is_dir() {
        return Err(format!(
            "frontend project is not a directory: {}",
            project.display()
        ));
    }
    Ok(project)
}

fn ensure_gitignore(project: &Path) -> Result<(), String> {
    let path = project.join(".gitignore");
    reject_symlink_path(project, &path)?;
    let mut source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if source.lines().any(|line| line.trim() == GITIGNORE_ENTRY) {
        return Ok(());
    }
    if !source.is_empty() && !source.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(GITIGNORE_ENTRY);
    source.push('\n');
    atomic_write(project, &path, source.as_bytes())
}

fn reject_symlink_path(project: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(project)
        .map_err(|_| format!("local nlab-api path is outside project: {}", path.display()))?;
    let mut current = project.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refuse to write through symlinked path: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(format!("cannot inspect {}: {error}", current.display())),
        }
    }
    Ok(())
}

fn atomic_write(project: &Path, path: &Path, content: &[u8]) -> Result<(), String> {
    reject_symlink_path(project, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("local nlab-api path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    reject_symlink_path(project, path)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("cannot stage {}: {error}", path.display()))?;
    temporary
        .write_all(content)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o7777)
            .unwrap_or(0o644);
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|error| format!("cannot set permissions for {}: {error}", path.display()))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| format!("cannot replace {}: {}", path.display(), error.error))?;
    Ok(())
}

fn standalone_arguments(arguments: &[OsString]) -> Option<&[OsString]> {
    (arguments
        .get(1)
        .is_some_and(|argument| argument == OsStr::new("nlab-api")))
    .then(|| &arguments[2..])
}

fn project_argument(arguments: &[OsString]) -> Result<PathBuf, String> {
    let mut project = PathBuf::from(".");
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--project" {
            index += 1;
            let value = arguments
                .get(index)
                .ok_or_else(|| "--project requires a path".to_owned())?;
            project = PathBuf::from(value);
        } else if let Some(value) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix("--project="))
        {
            project = PathBuf::from(value);
        }
        index += 1;
    }
    Ok(project)
}

fn execute_standalone(arguments: &[OsString]) -> Result<u8, String> {
    let status = Command::new("nlab-api")
        .args(arguments)
        .status()
        .map_err(|error| {
            format!(
                "cannot run standalone nlab-api: {error}; install nlab-api or configure jt with `jt nlab-api config --runner jt --project <path>`"
            )
        })?;
    Ok(status.code().unwrap_or(1).clamp(0, u8::MAX as i32) as u8)
}

#[cfg(test)]
mod tests {
    use super::{GITIGNORE_ENTRY, Runner, configure_inner, runner};
    use std::fs;

    #[test]
    fn local_runner_config_is_ignored_and_unset_restores_default() {
        let project = tempfile::tempdir().unwrap();
        let configured = configure_inner(super::ConfigArgs {
            project: project.path().to_path_buf(),
            runner: Some(super::ConfiguredRunner::Jt),
            unset: false,
        })
        .unwrap();
        assert!(configured.contains("configured as jt"));
        assert_eq!(runner(project.path()).unwrap(), Runner::Jt);
        assert_eq!(
            fs::read_to_string(project.path().join(".gitignore")).unwrap(),
            format!("{GITIGNORE_ENTRY}\n")
        );

        configure_inner(super::ConfigArgs {
            project: project.path().to_path_buf(),
            runner: None,
            unset: true,
        })
        .unwrap();
        assert_eq!(runner(project.path()).unwrap(), Runner::Standalone);
        assert!(!project.path().join(".nlab/cli.local.json").exists());
    }
}
