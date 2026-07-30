use std::{collections::BTreeMap, ffi::OsString, path::Path};

use serde_json::Value;

use crate::node::{
    command::{CommandResult, CommandSpec, Runner},
    error::{AppError, Result},
    model::{PACKAGE_REGISTRY, ZZ_REGISTRY},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NrmState {
    pub sources: BTreeMap<String, String>,
    pub current: Option<String>,
}

pub fn configure_taobao_and_zz(
    runner: &dyn Runner,
    nrm: &Path,
    environment: &BTreeMap<OsString, OsString>,
    nrmrc: &Path,
) -> Result<NrmState> {
    let state = read_state(runner, nrm, environment)?;
    let zz_configured = state
        .sources
        .get("zz")
        .is_some_and(|url| normalize_url(url) == ZZ_REGISTRY);
    let zz_url_present = state
        .sources
        .values()
        .any(|url| normalize_url(url) == ZZ_REGISTRY);
    if !zz_configured && !zz_url_present {
        if state.sources.contains_key("zz") {
            run_nrm(
                runner,
                nrm,
                environment,
                ["del", "zz"],
                "remove old zz source",
            )?;
        }
        run_nrm(
            runner,
            nrm,
            environment,
            ["add", "zz", ZZ_REGISTRY],
            "add zz source",
        )?;
    }
    if nrmrc_overrides_taobao(nrmrc) {
        run_nrm(
            runner,
            nrm,
            environment,
            ["del", "taobao"],
            "remove custom taobao source",
        )?;
    }
    run_nrm(
        runner,
        nrm,
        environment,
        ["use", "taobao"],
        "select taobao source",
    )?;
    let verified = read_state(runner, nrm, environment)?;
    if verified.current.as_deref() != Some("taobao")
        || verified
            .sources
            .get("taobao")
            .is_some_and(|url| normalize_url(url) != PACKAGE_REGISTRY)
    {
        return Err(AppError::Invalid(
            "nrm taobao source is not https://registry.npmmirror.com/".to_owned(),
        ));
    }
    Ok(verified)
}

pub fn parse_nrm_list(value: &str) -> NrmState {
    let mut state = NrmState::default();
    for line in value.lines() {
        let line = line.trim();
        let current = line.starts_with('*');
        let line = line.trim_start_matches('*').trim();
        let Some((name, url)) = split_registry_line(line) else {
            continue;
        };
        state.sources.insert(name.to_owned(), normalize_url(url));
        if current {
            state.current = Some(name.to_owned());
        }
    }
    state
}

pub fn nrmrc_overrides_taobao(path: &Path) -> bool {
    let Ok(value) = std::fs::read_to_string(path) else {
        return false;
    };
    if let Ok(value) = serde_json::from_str::<Value>(&value) {
        return json_has_taobao(&value);
    }
    value.lines().any(|line| {
        let line = line.trim();
        line.starts_with("taobao") && (line.contains(':') || line.contains('='))
    })
}

fn read_state(
    runner: &dyn Runner,
    nrm: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<NrmState> {
    let output = run_nrm(runner, nrm, environment, ["ls"], "read nrm sources")?;
    let state = parse_nrm_list(&output.stdout);
    if state.sources.is_empty() {
        return Err(AppError::Decode {
            action: "read nrm sources".to_owned(),
            detail: "no source row found".to_owned(),
        });
    }
    Ok(state)
}

fn run_nrm<const N: usize>(
    runner: &dyn Runner,
    nrm: &Path,
    environment: &BTreeMap<OsString, OsString>,
    args: [&str; N],
    action: &str,
) -> Result<CommandResult> {
    let mut command = CommandSpec::new(nrm, args);
    command.env = environment.clone();
    command.clear_env = true;
    let result = runner.run(&command)?;
    result.require_success(action, &[])?;
    Ok(result)
}

fn split_registry_line(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.split_whitespace();
    let name = parts.next()?;
    let separator = parts.next()?;
    let url = parts.next()?;
    (separator.len() >= 3 && separator.bytes().all(|byte| byte == b'-') && url.starts_with("http"))
        .then_some((name, url))
}

fn normalize_url(value: &str) -> String {
    format!("{}/", value.trim().trim_end_matches('/'))
}

fn json_has_taobao(value: &Value) -> bool {
    match value {
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| key == "taobao" || json_has_taobao(value)),
        Value::Array(values) => values.iter().any(json_has_taobao),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString, path::Path, sync::Mutex};

    use tempfile::tempdir;

    use crate::node::{
        command::{CommandResult, CommandSpec, Runner},
        error::Result,
    };

    use super::{configure_taobao_and_zz, nrmrc_overrides_taobao, parse_nrm_list};

    #[test]
    fn parses_current_human_nrm_output() {
        let state = parse_nrm_list(
            "  npm ---------- https://registry.npmjs.org/\n* taobao ------- https://registry.npmmirror.com/\n  zz ----------- https://rcnpm.zhuanspirit.com/\n",
        );

        assert_eq!(state.current.as_deref(), Some("taobao"));
        assert_eq!(
            state.sources.get("taobao").map(String::as_str),
            Some("https://registry.npmmirror.com/")
        );
        assert_eq!(
            state.sources.get("zz").map(String::as_str),
            Some("https://rcnpm.zhuanspirit.com/")
        );
    }

    #[test]
    fn recognizes_custom_taobao_in_nrmrc_only() {
        let root = tempdir().unwrap();
        let config = root.path().join(".nrmrc");
        std::fs::write(&config, r#"{"registries":{"taobao":"https://wrong/"}}"#).unwrap();

        assert!(nrmrc_overrides_taobao(&config));
    }

    #[test]
    fn configures_zz_and_selects_builtin_taobao_without_adding_taobao() {
        let root = tempdir().unwrap();
        let nrmrc = root.path().join(".nrmrc");
        std::fs::write(&nrmrc, r#"{"taobao":"https://wrong/"}"#).unwrap();
        let runner = NrmRunner::default();

        configure_taobao_and_zz(
            &runner,
            Path::new("/nlab/nrm"),
            &BTreeMap::from([(OsString::from("PATH"), OsString::from("/nlab"))]),
            &nrmrc,
        )
        .unwrap();

        let commands = runner.commands.lock().unwrap();
        assert!(
            commands
                .iter()
                .any(|args| args == &vec!["add", "zz", "https://rcnpm.zhuanspirit.com/"])
        );
        assert!(commands.iter().any(|args| args == &vec!["use", "taobao"]));
        assert!(
            !commands
                .iter()
                .any(|args| args == &vec!["add", "taobao", "https://registry.npmmirror.com/"])
        );
    }

    #[test]
    fn skips_zz_add_when_registry_url_already_exists() {
        let root = tempdir().unwrap();
        let runner = NrmRunner::with_list(
            "  corp --------- https://rcnpm.zhuanspirit.com/\n* taobao ------- https://registry.npmmirror.com/\n",
        );

        configure_taobao_and_zz(
            &runner,
            Path::new("/nlab/nrm"),
            &BTreeMap::new(),
            &root.path().join(".nrmrc"),
        )
        .unwrap();

        let commands = runner.commands.lock().unwrap();
        assert!(
            !commands
                .iter()
                .any(|args| args.first().is_some_and(|arg| arg == "add"))
        );
        assert!(commands.iter().any(|args| args == &vec!["use", "taobao"]));
    }

    struct NrmRunner {
        commands: Mutex<Vec<Vec<String>>>,
        list: &'static str,
    }

    impl Default for NrmRunner {
        fn default() -> Self {
            Self::with_list(
                "  zz ----------- https://wrong/\n* taobao ------- https://registry.npmmirror.com/\n",
            )
        }
    }

    impl NrmRunner {
        fn with_list(list: &'static str) -> Self {
            Self {
                commands: Mutex::new(Vec::new()),
                list,
            }
        }
    }

    impl Runner for NrmRunner {
        fn run(&self, command: &CommandSpec) -> Result<CommandResult> {
            let args = command
                .args
                .iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            self.commands.lock().unwrap().push(args.clone());
            let stdout = if args == ["ls"] {
                self.list.to_owned()
            } else {
                String::new()
            };
            Ok(CommandResult {
                status: 0,
                stdout,
                stderr: String::new(),
            })
        }
    }
}
