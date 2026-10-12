use std::{collections::BTreeMap, ffi::OsString, path::Path};

use crate::node::{
    command::{CommandResult, CommandSpec, Runner},
    error::{AppError, Result},
    model::ZZ_REGISTRY,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NrmState {
    pub sources: BTreeMap<String, String>,
    pub current: Option<String>,
}

pub fn configure_registry_and_zz(
    runner: &dyn Runner,
    nrm: &Path,
    environment: &BTreeMap<OsString, OsString>,
    registry: &str,
) -> Result<NrmState> {
    let mut state = read_state(runner, nrm, environment)?;
    let zz_url_present = state
        .sources
        .values()
        .any(|url| normalize_url(url) == ZZ_REGISTRY);
    if !zz_url_present {
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
        state = read_state(runner, nrm, environment)?;
    }
    let registry = normalize_url(registry);
    let source = if let Some((name, _)) = state
        .sources
        .iter()
        .find(|(_, url)| normalize_url(url) == registry)
    {
        name.clone()
    } else {
        if state.sources.contains_key("jt") {
            run_nrm(
                runner,
                nrm,
                environment,
                ["del", "jt"],
                "remove old jt source",
            )?;
        }
        run_nrm(
            runner,
            nrm,
            environment,
            ["add", "jt", &registry],
            "add jt source",
        )?;
        "jt".to_owned()
    };
    run_nrm(
        runner,
        nrm,
        environment,
        ["use", &source],
        "select npm registry",
    )?;
    let verified = read_state(runner, nrm, environment)?;
    if verified
        .current
        .as_ref()
        .and_then(|current| verified.sources.get(current))
        .is_none_or(|url| normalize_url(url) != registry)
    {
        return Err(AppError::Invalid(format!(
            "nrm current registry does not match {registry}"
        )));
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
    let normalized = format!("{}/", value.trim().trim_end_matches('/'));
    url::Url::parse(&normalized)
        .map(|url| url.to_string())
        .unwrap_or(normalized)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, VecDeque},
        ffi::OsString,
        path::Path,
        sync::Mutex,
    };

    use crate::node::{
        command::{CommandResult, CommandSpec, Runner},
        error::Result,
    };

    use super::{configure_registry_and_zz, parse_nrm_list};

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
    fn canonicalizes_registry_hosts_and_ports_without_changing_path_case() {
        let state = parse_nrm_list(
            "  upper -------- https://PACKAGES.EXAMPLE:443/NPM\n  lower -------- https://packages.example/npm/\n",
        );
        assert_eq!(state.sources["upper"], "https://packages.example/NPM/");
        assert_eq!(state.sources["lower"], "https://packages.example/npm/");
    }

    #[test]
    fn configures_zz_and_selects_builtin_taobao_without_adding_taobao() {
        let runner = NrmRunner::with_lists([
            "  zz ----------- https://wrong/\n* taobao ------- https://registry.npmmirror.com/\n",
            "  zz ----------- https://rcnpm.zhuanspirit.com/\n* taobao ------- https://registry.npmmirror.com/\n",
            "  zz ----------- https://rcnpm.zhuanspirit.com/\n* taobao ------- https://registry.npmmirror.com/\n",
        ]);

        configure_registry_and_zz(
            &runner,
            Path::new("/nlab/nrm"),
            &BTreeMap::from([(OsString::from("PATH"), OsString::from("/nlab"))]),
            crate::node::model::PACKAGE_REGISTRY,
        )
        .unwrap();

        let commands = runner.commands.lock().unwrap();
        assert_eq!(
            *commands,
            vec![
                vec!["ls"],
                vec!["del", "zz"],
                vec!["add", "zz", "https://rcnpm.zhuanspirit.com/"],
                vec!["ls"],
                vec!["use", "taobao"],
                vec!["ls"],
            ]
        );
    }

    #[test]
    fn skips_zz_add_when_registry_url_already_exists() {
        let list = "  corp --------- https://rcnpm.zhuanspirit.com/\n* taobao ------- https://registry.npmmirror.com/\n";
        let runner = NrmRunner::with_lists([list, list]);

        configure_registry_and_zz(
            &runner,
            Path::new("/nlab/nrm"),
            &BTreeMap::new(),
            crate::node::model::PACKAGE_REGISTRY,
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

    #[test]
    fn selects_official_and_custom_registry_and_verifies_selection() {
        for (registry, source, list) in [
            (
                "https://registry.npmjs.org/",
                "npm",
                "  zz ----------- https://rcnpm.zhuanspirit.com/\n* npm ---------- https://registry.npmjs.org/\n",
            ),
            (
                "https://packages.example/npm/",
                "corp",
                "  zz ----------- https://rcnpm.zhuanspirit.com/\n* corp --------- https://PACKAGES.EXAMPLE:443/npm\n",
            ),
            (
                "https://rcnpm.zhuanspirit.com/",
                "zz",
                "* zz ----------- https://rcnpm.zhuanspirit.com/\n",
            ),
        ] {
            let runner = NrmRunner::with_lists([list, list]);
            let state = configure_registry_and_zz(
                &runner,
                Path::new("/nlab/nrm"),
                &BTreeMap::new(),
                registry,
            )
            .unwrap();
            assert_eq!(state.current.as_deref(), Some(source));
            assert!(
                runner
                    .commands
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|args| args == &vec!["use", source])
            );
            assert!(
                runner
                    .commands
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|args| args[0] != "add")
            );
        }
        let runner = NrmRunner::with_lists([
            "  zz ----------- https://rcnpm.zhuanspirit.com/\n  npm ---------- https://registry.npmjs.org/\n* taobao ------- https://registry.npmmirror.com/\n",
            "  zz ----------- https://rcnpm.zhuanspirit.com/\n  npm ---------- https://registry.npmjs.org/\n* taobao ------- https://registry.npmmirror.com/\n",
        ]);
        assert!(
            configure_registry_and_zz(
                &runner,
                Path::new("/nlab/nrm"),
                &BTreeMap::new(),
                "https://registry.npmjs.org/"
            )
            .is_err()
        );
    }

    #[test]
    fn replaces_custom_registry_before_selecting_it() {
        let runner = NrmRunner::with_lists([
            "  zz ----------- https://rcnpm.zhuanspirit.com/\n* jt ----------- https://old.example/\n",
            "  zz ----------- https://rcnpm.zhuanspirit.com/\n* jt ----------- https://packages.example/npm/\n",
        ]);
        configure_registry_and_zz(
            &runner,
            Path::new("/nlab/nrm"),
            &BTreeMap::new(),
            "https://packages.example/npm/",
        )
        .unwrap();
        assert_eq!(
            *runner.commands.lock().unwrap(),
            vec![
                vec!["ls"],
                vec!["del", "jt"],
                vec!["add", "jt", "https://packages.example/npm/"],
                vec!["use", "jt"],
                vec!["ls"]
            ]
        );
    }

    #[test]
    fn uses_freshly_added_zz_source_instead_of_adding_duplicate_jt() {
        let runner = NrmRunner::with_lists([
            "* npm ---------- https://registry.npmjs.org/\n",
            "* npm ---------- https://registry.npmjs.org/\n  zz ----------- https://rcnpm.zhuanspirit.com/\n",
            "  npm ---------- https://registry.npmjs.org/\n* zz ----------- https://rcnpm.zhuanspirit.com/\n",
        ]);
        configure_registry_and_zz(
            &runner,
            Path::new("/nlab/nrm"),
            &BTreeMap::new(),
            "https://rcnpm.zhuanspirit.com/",
        )
        .unwrap();
        assert_eq!(
            *runner.commands.lock().unwrap(),
            vec![
                vec!["ls"],
                vec!["add", "zz", "https://rcnpm.zhuanspirit.com/"],
                vec!["ls"],
                vec!["use", "zz"],
                vec!["ls"],
            ]
        );
    }

    #[test]
    fn preserves_builtin_overrides_and_adds_missing_registry_as_jt() {
        for (registry, before, after) in [
            (
                "https://registry.npmjs.org/",
                "* npm ---------- https://custom.example/\n  zz ----------- https://rcnpm.zhuanspirit.com/\n",
                "  npm ---------- https://custom.example/\n  zz ----------- https://rcnpm.zhuanspirit.com/\n* jt ----------- https://registry.npmjs.org/\n",
            ),
            (
                "https://registry.npmmirror.com/",
                "* taobao ------- https://custom.example/\n  zz ----------- https://rcnpm.zhuanspirit.com/\n",
                "  taobao ------- https://custom.example/\n  zz ----------- https://rcnpm.zhuanspirit.com/\n* jt ----------- https://registry.npmmirror.com/\n",
            ),
        ] {
            let runner = NrmRunner::with_lists([before, after]);
            configure_registry_and_zz(&runner, Path::new("/nlab/nrm"), &BTreeMap::new(), registry)
                .unwrap();
            assert_eq!(
                *runner.commands.lock().unwrap(),
                vec![
                    vec!["ls"],
                    vec!["add", "jt", registry],
                    vec!["use", "jt"],
                    vec!["ls"],
                ]
            );
        }
    }

    #[test]
    fn accepts_multiple_current_aliases_for_same_registry_url() {
        let list = "  zz ----------- https://rcnpm.zhuanspirit.com/\n* corp --------- https://packages.example/npm/\n* jt ----------- https://packages.example/npm/\n";
        let runner = NrmRunner::with_lists([list, list]);
        let state = configure_registry_and_zz(
            &runner,
            Path::new("/nlab/nrm"),
            &BTreeMap::new(),
            "https://packages.example/npm/",
        )
        .unwrap();
        assert_eq!(state.current.as_deref(), Some("jt"));
        assert_eq!(
            *runner.commands.lock().unwrap(),
            vec![vec!["ls"], vec!["use", "corp"], vec!["ls"]]
        );
    }

    struct NrmRunner {
        commands: Mutex<Vec<Vec<String>>>,
        lists: Mutex<VecDeque<&'static str>>,
    }

    impl NrmRunner {
        fn with_lists<const N: usize>(lists: [&'static str; N]) -> Self {
            Self {
                commands: Mutex::new(Vec::new()),
                lists: Mutex::new(VecDeque::from(lists)),
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
                self.lists
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected nrm ls")
                    .to_owned()
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
