use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::PathBuf,
    process::Command,
};

use crate::node::error::{AppError, Result};

const PROXY_KEYS: [&str; 8] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<OsString, OsString>,
    pub remove_env: BTreeSet<OsString>,
    pub clear_env: bool,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            env: BTreeMap::new(),
            remove_env: BTreeSet::new(),
            clear_env: false,
        }
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    pub fn require_success(&self, action: &str, secret_values: &[String]) -> Result<()> {
        if self.success() {
            return Ok(());
        }
        let detail = self
            .stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .or_else(|| {
                self.stdout
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
            })
            .unwrap_or("command returned non-zero");
        Err(AppError::Command {
            action: action.to_owned(),
            status: self.status,
            detail: redact(detail, secret_values),
        })
    }
}

pub trait Runner: Send + Sync {
    fn run(&self, command: &CommandSpec) -> Result<CommandResult>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, command: &CommandSpec) -> Result<CommandResult> {
        let mut child = Command::new(&command.program);
        child.args(&command.args);
        if let Some(cwd) = &command.cwd {
            child.current_dir(cwd);
        }
        if command.clear_env {
            child.env_clear();
        }
        child.envs(&command.env);
        for key in &command.remove_env {
            child.env_remove(key);
        }

        let output = child.output().map_err(|source| {
            AppError::io("start command", Some(command.program.clone()), source)
        })?;
        Ok(CommandResult {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

pub fn inherited_proxy_env(source: &BTreeMap<OsString, OsString>) -> BTreeMap<OsString, OsString> {
    PROXY_KEYS
        .iter()
        .filter_map(|key| {
            source
                .get(&OsString::from(key))
                .filter(|value| !value.is_empty())
                .map(|value| (OsString::from(key), value.clone()))
        })
        .collect()
}

pub fn redact(value: &str, secret_values: &[String]) -> String {
    let mut result = value.to_owned();
    for secret in secret_values {
        if !secret.is_empty() {
            result = result.replace(secret, "***");
        }
    }
    result
        .split_whitespace()
        .map(redact_url_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_url_token(token: &str) -> String {
    let Some(scheme_end) = token.find("://") else {
        return token.to_owned();
    };
    let prefix_end = scheme_end + 3;
    let without_query = token
        .find(['?', '#'])
        .map(|index| &token[..index])
        .unwrap_or(token);
    let Some(at) = without_query[prefix_end..].find('@') else {
        return without_query.to_owned();
    };
    let at = prefix_end + at;
    format!(
        "{}***@{}",
        &without_query[..prefix_end],
        &without_query[at + 1..]
    )
}

pub fn os_env() -> BTreeMap<OsString, OsString> {
    std::env::vars_os().collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString};

    use super::{CommandSpec, Runner, SystemRunner, inherited_proxy_env, redact};

    #[test]
    fn keeps_only_standard_proxy_variables() {
        let environment = BTreeMap::from([
            (
                OsString::from("HTTP_PROXY"),
                OsString::from("http://user:pass@proxy:7890"),
            ),
            (OsString::from("PATH"), OsString::from("/bin")),
        ]);

        let proxy = inherited_proxy_env(&environment);

        assert_eq!(proxy.len(), 1);
        assert!(proxy.contains_key(&OsString::from("HTTP_PROXY")));
    }

    #[test]
    fn redacts_known_proxy_and_url_credentials() {
        let secret = "http://user:pass@proxy:7890".to_owned();
        let value = redact(
            "curl http://user:pass@example.test/a?token=secret via http://user:pass@proxy:7890",
            &[secret],
        );

        assert!(!value.contains("pass"));
        assert!(!value.contains("token=secret"));
    }

    #[test]
    fn clear_env_does_not_leak_parent_environment() {
        let mut command = CommandSpec::new("/usr/bin/env", Vec::<String>::new());
        command.clear_env = true;
        command
            .env
            .insert(OsString::from("JT_TEST_ONLY"), OsString::from("kept"));

        let result = SystemRunner.run(&command).unwrap();

        assert_eq!(result.stdout, "JT_TEST_ONLY=kept\n");
    }
}
