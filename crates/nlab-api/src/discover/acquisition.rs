use super::*;
use serde_json::Value;
use std::process::{Command, Stdio};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Acquisition {
    pub repository: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

pub(super) fn scan(args: DiscoverArgs, deadline: Instant) -> Result<DiscoveryRun> {
    scan_with(
        args,
        |path| repo::sync_index(path, deadline),
        |service, root| {
            let mut acquisition = Acquisition {
                repository: None,
                status: "acquisition-failed".to_owned(),
                error: None,
            };
            let outcome = (|| {
                let (url, name) = resolve_repository(service, deadline)?;
                acquisition.repository = Some(url.clone());
                repo::clone_missing(&url, &root.join(name), deadline)
            })();
            match outcome {
                Ok(()) => acquisition.status = "cloned".to_owned(),
                Err(error) => acquisition.error = Some(format!("{error:#}")),
            }
            acquisition
        },
    )
}

pub(super) fn scan_with(
    args: DiscoverArgs,
    mut sync: impl FnMut(&Path) -> Result<()>,
    mut acquire: impl FnMut(&str, &Path) -> Acquisition,
) -> Result<DiscoveryRun> {
    let config = ProjectConfig::load(&args.project)?;
    let root = args
        .repositories_root
        .clone()
        .or(LocalProjectConfig::load(&args.project)?
            .backend
            .repositories_root)
        .context("repositories root missing; pass --repositories-root <path>")?
        .canonicalize()?;
    let backend = repo::resolve_path(
        &config.backend.repo_path,
        config.backend.repository.as_deref(),
    )?
    .canonicalize()?;
    let previous = config.discovery.unwrap_or_default();
    let mut synchronized = BTreeMap::new();
    let mut acquisitions = BTreeMap::new();
    loop {
        let mut paths = fs::read_dir(&root)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.retain(|path| path.join(".git").exists());
        paths.push(backend.clone());
        for path in paths {
            let path = path.canonicalize()?;
            if let std::collections::btree_map::Entry::Vacant(entry) =
                synchronized.entry(path.clone())
            {
                entry.insert(sync(&path).map_err(|error| format!("{error:#}")));
            }
        }
        let mut result = super::scan(args.clone(), &synchronized)?;
        let missing = result
            .report
            .calls
            .iter()
            .filter(|call| call.status == "missing-source" && call.depth < args.max_depth)
            .filter(|call| {
                call.bindings
                    .iter()
                    .map(|binding| &binding.service)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == 1
            })
            .flat_map(|call| call.bindings.iter().map(|binding| binding.service.clone()))
            .filter(|service| {
                !args.offline
                    && !acquisitions.contains_key(service)
                    && !args.allow_missing.contains(service)
                    && !previous
                        .services
                        .get(service)
                        .is_some_and(|state| state.allow_missing)
            })
            .collect::<BTreeSet<_>>();
        if missing.is_empty() {
            result.report.acquisitions = acquisitions;
            return Ok(result);
        }
        let mut cloned = false;
        for service in missing {
            let acquisition = acquire(&service, &root);
            cloned |= acquisition.status == "cloned";
            acquisitions.insert(service, acquisition);
        }
        if !cloned {
            result.report.acquisitions = acquisitions;
            return Ok(result);
        }
    }
}

fn sic(command: &str, parameter: &str, value: &str, deadline: Instant) -> Result<Value> {
    // File-backed output avoids a pipe filling while the deadline runner waits.
    let output = tempfile::tempfile()?;
    let mut process = Command::new("zzcli");
    process
        .args(["sic", command, parameter, value])
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(Stdio::null());
    let status = repo::run_until(&mut process, deadline)?;
    if !status.success() {
        bail!(
            "SIC {command} failed with status {status}; check zzcli authentication and permissions"
        );
    }
    use std::io::{Read, Seek};
    let mut output = output;
    output.rewind()?;
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes)?;
    let response: Value = serde_json::from_slice(&bytes).context("decode SIC response")?;
    if response["code"] != 0 || !response["result"].is_object() {
        bail!(
            "SIC {command} returned no successful service record (code {})",
            response["code"]
        );
    }
    Ok(response["result"].clone())
}

fn resolve_repository(service: &str, deadline: Instant) -> Result<(String, String)> {
    let app = sic(
        "get-cluster-info-by-app-name",
        "--appName",
        service,
        deadline,
    )?;
    let cluster = app["clusterName"]
        .as_str()
        .filter(|name| !name.is_empty())
        .context("SIC clusterName missing")?;
    let details = sic(
        "get-cluster-info-with-group",
        "--clusterName",
        cluster,
        deadline,
    )?;
    repository_from_details(&details)
}

fn repository_from_details(details: &Value) -> Result<(String, String)> {
    let info = match &details["beetleInfo"] {
        Value::String(json) => {
            serde_json::from_str::<Value>(json).context("decode SIC beetleInfo")?
        }
        value => value.clone(),
    };
    let group = info["groupName"]
        .as_str()
        .context("SIC repository groupName missing")?;
    let name = info["projectName"]
        .as_str()
        .context("SIC repository projectName missing")?;
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.starts_with('-')
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    };
    if !group.split('/').all(valid) || !valid(name) {
        bail!("invalid SIC repository groupName/projectName");
    }
    Ok((
        format!("git@gitlab.zhuanspirit.com:{group}/{name}.git"),
        name.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sic_repository_fields_accept_json_and_reject_path_escape() {
        for info in [
            serde_json::json!({"groupName":"zz-youpin", "projectName":"n_lab_scm_goods"}),
            Value::String(
                r#"{"groupName":"zz-youpin","projectName":"n_lab_scm_goods"}"#.to_owned(),
            ),
        ] {
            assert_eq!(
                repository_from_details(&serde_json::json!({"beetleInfo":info})).unwrap(),
                (
                    "git@gitlab.zhuanspirit.com:zz-youpin/n_lab_scm_goods.git".to_owned(),
                    "n_lab_scm_goods".to_owned()
                )
            );
        }
        for name in ["../escape", ".", "..", "-option", "/absolute", "x; command"] {
            assert!(
                repository_from_details(
                    &serde_json::json!({"beetleInfo":{"groupName":"zz-youpin","projectName":name}})
                )
                .is_err()
            );
        }
        assert!(repository_from_details(&serde_json::json!({"beetleInfo":{}})).is_err());
    }
}
