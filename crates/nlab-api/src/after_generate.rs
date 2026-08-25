use std::path::Path;
use std::process::Command;
use std::time::Instant;

use serde::Serialize;

use crate::config::AfterGenerateHook;

const GENERATED_FILES_ENV: &str = "NLAB_API_GENERATED_FILES";
const PROJECT_ROOT_ENV: &str = "NLAB_API_PROJECT_ROOT";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookReport {
    command: Vec<String>,
    duration_ms: u128,
    exit_status: Option<i32>,
}

#[derive(Debug)]
pub struct HookFailure {
    pub reports: Vec<HookReport>,
    pub message: String,
}

pub fn run(
    project: &Path,
    hooks: &[AfterGenerateHook],
    generated_files: &[String],
) -> Result<Vec<HookReport>, HookFailure> {
    let files_json = serde_json::to_string(generated_files).expect("serialize generated file list");
    let mut reports = Vec::new();
    for hook in hooks {
        let mut args = hook.args.clone();
        if hook.include_generated_files {
            args.extend_from_slice(generated_files);
        }
        let command = std::iter::once(hook.command.clone())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();
        let started = Instant::now();
        let output = Command::new(&hook.command)
            .args(&args)
            .current_dir(project)
            .env(GENERATED_FILES_ENV, &files_json)
            .env(PROJECT_ROOT_ENV, project)
            .output();
        let duration_ms = started.elapsed().as_millis();
        match output {
            Ok(output) => {
                reports.push(HookReport {
                    command,
                    duration_ms,
                    exit_status: output.status.code(),
                });
                if !output.status.success() {
                    let status = output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_owned(), |code| code.to_string());
                    return Err(HookFailure {
                        reports,
                        message: format!(
                            "{} exited with {status}: {}",
                            hook.command,
                            last_nonempty_line(&output.stderr, &output.stdout)
                        ),
                    });
                }
            }
            Err(error) => {
                reports.push(HookReport {
                    command,
                    duration_ms,
                    exit_status: None,
                });
                return Err(HookFailure {
                    reports,
                    message: format!("could not start {}: {error}", hook.command),
                });
            }
        }
    }
    Ok(reports)
}

fn last_nonempty_line(stderr: &[u8], stdout: &[u8]) -> String {
    [stderr, stdout]
        .into_iter()
        .find_map(|content| {
            content
                .split(|byte| *byte == b'\n')
                .rev()
                .find(|line| !line.iter().all(u8::is_ascii_whitespace))
        })
        .map(|line| String::from_utf8_lossy(line).chars().take(500).collect())
        .unwrap_or_else(|| "command returned non-zero".to_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn passes_project_and_exact_generated_files_without_shell_expansion() {
        let project = tempfile::tempdir().unwrap();
        let hook = AfterGenerateHook {
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf '%s\n%s\n' \"$NLAB_API_PROJECT_ROOT\" \"$NLAB_API_GENERATED_FILES\" > hook.txt"
                    .to_owned(),
            ],
            include_generated_files: false,
        };
        let files = vec!["src/api/a.ts".to_owned(), "src/types/a.ts".to_owned()];

        let reports = run(project.path(), &[hook], &files).unwrap();

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].exit_status, Some(0));
        assert_eq!(
            std::fs::read_to_string(project.path().join("hook.txt")).unwrap(),
            format!(
                "{}\n{}\n",
                project.path().display(),
                serde_json::to_string(&files).unwrap()
            )
        );
    }

    #[test]
    fn stops_after_nonzero_and_keeps_failed_report() {
        let project = tempfile::tempdir().unwrap();
        let hooks = [
            AfterGenerateHook {
                command: "sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "printf 'lint failed\\n' >&2; exit 7".to_owned(),
                ],
                include_generated_files: false,
            },
            AfterGenerateHook {
                command: "sh".to_owned(),
                args: vec!["-c".to_owned(), "touch should-not-run".to_owned()],
                include_generated_files: false,
            },
        ];

        let failure = run(project.path(), &hooks, &[]).unwrap_err();

        assert_eq!(failure.reports.len(), 1);
        assert_eq!(failure.reports[0].exit_status, Some(7));
        assert!(failure.message.contains("lint failed"));
        assert!(!project.path().join("should-not-run").exists());
    }

    #[test]
    fn appends_generated_files_as_literal_arguments_when_requested() {
        let project = tempfile::tempdir().unwrap();
        let hook = AfterGenerateHook {
            command: "sh".to_owned(),
            args: vec!["-c".to_owned(), "exit 0".to_owned(), "hook".to_owned()],
            include_generated_files: true,
        };
        let files = vec!["src/api/a file.ts".to_owned(), "src/types/a.ts".to_owned()];

        let reports = run(project.path(), &[hook], &files).unwrap();

        assert_eq!(
            reports[0].command,
            vec![
                "sh",
                "-c",
                "exit 0",
                "hook",
                "src/api/a file.ts",
                "src/types/a.ts"
            ]
        );
    }
}
