use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Runner {
    NlabApi,
    Jt,
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
        Runner::NlabApi => execute_standalone(arguments).map(Some),
        Runner::Jt => Ok(None),
    }
}

fn runner(project: &Path) -> Result<Runner, String> {
    let project = resolve_project(project)?;
    let config = nlab_api::LocalProjectConfig::load(&project).map_err(|error| error.to_string())?;
    Ok(match config.runner {
        Some(nlab_api::LocalRunner::Jt) => Runner::Jt,
        Some(nlab_api::LocalRunner::NlabApi) | None => Runner::NlabApi,
    })
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
    use super::{Runner, runner};

    #[test]
    fn local_runner_config_maps_both_explicit_values() {
        let project = tempfile::tempdir().unwrap();
        let mut local = nlab_api::LocalProjectConfig::default();
        local.backend.repo_path = Some(project.path().join("backend"));
        local.save(project.path()).unwrap();
        assert_eq!(runner(project.path()).unwrap(), Runner::NlabApi);

        local.runner = Some(nlab_api::LocalRunner::Jt);
        local.save(project.path()).unwrap();
        assert_eq!(runner(project.path()).unwrap(), Runner::Jt);

        local.runner = Some(nlab_api::LocalRunner::NlabApi);
        local.save(project.path()).unwrap();
        assert_eq!(runner(project.path()).unwrap(), Runner::NlabApi);
    }
}
