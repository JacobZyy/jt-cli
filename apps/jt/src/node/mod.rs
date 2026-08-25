mod aggressive;
mod cleanup;
mod cli;
pub(crate) mod command;
mod context;
pub(crate) mod error;
pub(crate) mod fs;
mod inventory;
mod model;
mod nrm;
pub(crate) mod platform;
mod shell;

use std::io::IsTerminal;

use aggressive::{AggressiveCleanupPlan, GlobalStage};
use cli::TerminalPrompter;
use command::{SystemRunner, os_env};
use context::AppContext;
use error::{AppError, Result};
use model::{GlobalStatus, PackageSource, ReferenceImpact, StageOutcome, StageReport, StageStatus};
use platform::{HomePaths, supported_platform};

#[derive(Clone, Debug, Default)]
struct RunSummary {
    outcome: StageOutcome,
    stages: Vec<StageReport>,
    cancelled_before_mutation: bool,
}

impl RunSummary {
    fn exit_code(&self) -> u8 {
        u8::from(self.outcome.incomplete)
    }
}

pub fn init() -> u8 {
    if !std::io::stdin().is_terminal() {
        eprintln!("error: jt node init 仅支持交互运行");
        return 1;
    }

    let environment = os_env();
    let home = match HomePaths::from_environment(&environment) {
        Ok(home) => home,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let runner = SystemRunner;
    let mut prompt = TerminalPrompter::default();
    let mut context = AppContext {
        runner: &runner,
        prompt: &mut prompt,
        home,
        environment,
    };

    match run(&mut context) {
        Ok(summary) => {
            print_final(&summary);
            summary.exit_code()
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn run(context: &mut AppContext<'_>) -> Result<RunSummary> {
    supported_platform()?;
    shell::validate_zdotdir(&context.home.home, &context.environment)?;
    context.prompt.intro("jt node init")?;
    context.prompt.note(
        "激进模式计划",
        "安装 Vite+ 与 Node/pnpm 版本集合\n配置 nrm 与 shell loader\n迁移可重建 globals\nfresh 探测后再次确认，才清理旧环境",
    )?;
    if !context
        .prompt
        .confirm("确认开始安装与配置吗？（默认：否）")?
    {
        context.prompt.cancel("已取消；尚未修改环境。")?;
        return Ok(RunSummary {
            cancelled_before_mutation: true,
            ..RunSummary::default()
        });
    }
    run_aggressive(context)
}

fn run_aggressive(context: &mut AppContext<'_>) -> Result<RunSummary> {
    let mut stages = Vec::new();
    println!("阶段 1/4：安装 Vite+、默认工具链与版本集合。");
    let install = match aggressive::install(context) {
        Ok(install) => install,
        Err(error) => {
            return Ok(aggressive_failure(
                stages,
                "安装 Vite+ 与工具链",
                error,
                &["配置 nrm 与 shell", "迁移 globals", "清理旧环境"],
                "安装阶段已完成的 additive mutation 保留，未执行 destructive cleanup",
            ));
        }
    };
    stages.push(StageReport {
        name: "安装 Vite+ 与工具链".to_owned(),
        status: StageStatus::Completed,
        detail: format!(
            "Vite+ {}；default Node {}；pnpm {}",
            install.version, install.default_node, install.default_pnpm
        ),
    });

    println!("阶段 2/4：配置 nrm taobao/zz 与 Vite+ shell block。");
    if let Err(error) = aggressive::configure(context, &install) {
        return Ok(aggressive_failure(
            stages,
            "配置 nrm 与 shell",
            error,
            &["迁移 globals", "清理旧环境"],
            "Vite+ 与已安装 runtime/package 保留；配置错误详情含条件回滚结果",
        ));
    }
    stages.push(StageReport {
        name: "配置 nrm 与 shell".to_owned(),
        status: StageStatus::Completed,
        detail: "nrm taobao/zz 与 Vite+ shell block 已配置".to_owned(),
    });

    println!("阶段 3/4：fresh 探测旧 globals；单次批量 vp install -g。");
    let globals = match aggressive::configure_global_environment(context, &install) {
        Ok(globals) => globals,
        Err(error) => {
            return Ok(aggressive_failure(
                stages,
                "迁移 globals",
                error,
                &["清理旧环境"],
                "Vite+/runtime/nrm/shell 与已成功安装的 global 保留，未执行旧环境清理",
            ));
        }
    };
    render_globals(&globals);
    let failed_globals = globals
        .results
        .iter()
        .filter(|result| result.status != GlobalStatus::Installed)
        .count();
    stages.push(StageReport {
        name: "迁移 globals".to_owned(),
        status: if failed_globals == 0 {
            StageStatus::Completed
        } else {
            StageStatus::Partial
        },
        detail: format!(
            "{} 个候选，{} 个未达到目标",
            globals.results.len(),
            failed_globals
        ),
    });

    println!("阶段 4/4：fresh 探测旧环境与删除范围。");
    let plan = aggressive::cleanup_plan(context, &globals);
    println!("{}", cleanup_preview(&plan));
    if !plan.diagnostics.is_empty() {
        let mut outcome = StageOutcome::success("Vite+ 安装、配置与 global 迁移已完成");
        outcome.failure("fresh inventory 不完整；安全阻断 old-env cleanup");
        stages.push(StageReport {
            name: "清理旧环境".to_owned(),
            status: StageStatus::Skipped,
            detail: "inventory diagnostics 非空；未请求 destructive cleanup 授权".to_owned(),
        });
        return Ok(RunSummary {
            outcome,
            stages,
            cancelled_before_mutation: false,
        });
    }

    let confirmed = match context
        .prompt
        .confirm("确认执行以上 destructive cleanup 吗？（默认：否）")
    {
        Ok(confirmed) => confirmed,
        Err(error) => {
            return Ok(aggressive_failure(
                stages,
                "清理旧环境",
                error,
                &[],
                "未取得 cleanup 授权；新旧环境均保留",
            ));
        }
    };
    if !confirmed {
        let mut outcome = StageOutcome::success("Vite+ 安装、配置与 global 迁移已完成");
        outcome.failure("用户拒绝 old-env cleanup；新环境保留，旧环境未删除");
        stages.push(StageReport {
            name: "清理旧环境".to_owned(),
            status: StageStatus::Skipped,
            detail: "用户拒绝 destructive cleanup；旧环境未删除".to_owned(),
        });
        return Ok(RunSummary {
            outcome,
            stages,
            cancelled_before_mutation: false,
        });
    }

    let fresh_plan = aggressive::cleanup_plan(context, &globals);
    if !cleanup_revalidation_passed(&plan, &fresh_plan) {
        let mut outcome = StageOutcome::success("Vite+ 安装、配置与 global 迁移已完成");
        outcome.failure("cleanup 授权后环境发生变化或 inventory 不完整；未执行任何删除");
        stages.push(StageReport {
            name: "清理旧环境".to_owned(),
            status: StageStatus::Skipped,
            detail: "fresh revalidation 未通过；请重新运行 jt node init".to_owned(),
        });
        return Ok(RunSummary {
            outcome,
            stages,
            cancelled_before_mutation: false,
        });
    }

    if let Err(error) =
        aggressive::verify_vp_global_candidates(context, &install, &globals.inventory.candidates)
    {
        let mut outcome = StageOutcome::success("Vite+ 安装、配置与 global 迁移已完成");
        outcome.failure(format!(
            "Vite+ global fresh readback 未通过；未执行任何删除：{error}"
        ));
        stages.push(StageReport {
            name: "清理旧环境".to_owned(),
            status: StageStatus::Skipped,
            detail: "目标 globals 漂移或读取失败；请重新运行 jt node init".to_owned(),
        });
        return Ok(RunSummary {
            outcome,
            stages,
            cancelled_before_mutation: false,
        });
    }

    let outcome = aggressive::execute_cleanup(context, &fresh_plan);
    stages.push(StageReport {
        name: "清理旧环境".to_owned(),
        status: if outcome.incomplete {
            StageStatus::Partial
        } else {
            StageStatus::Completed
        },
        detail: format!(
            "{} 项完成，{} 项未完成；destructive mutation 不回滚",
            outcome.completed.len(),
            outcome.failures.len()
        ),
    });
    Ok(RunSummary {
        outcome,
        stages,
        cancelled_before_mutation: false,
    })
}

fn cleanup_revalidation_passed(
    previewed: &AggressiveCleanupPlan,
    fresh: &AggressiveCleanupPlan,
) -> bool {
    fresh.diagnostics.is_empty() && previewed.same_actions(fresh)
}

fn aggressive_failure(
    mut stages: Vec<StageReport>,
    name: &str,
    error: AppError,
    skipped: &[&str],
    retained: &str,
) -> RunSummary {
    stages.push(StageReport {
        name: name.to_owned(),
        status: StageStatus::Failed,
        detail: error.to_string(),
    });
    stages.extend(skipped.iter().map(|name| StageReport {
        name: (*name).to_owned(),
        status: StageStatus::Skipped,
        detail: "前置阶段失败，未执行".to_owned(),
    }));
    let mut outcome = StageOutcome::default();
    outcome.failure(format!("{name}失败：{error}"));
    outcome.note(retained);
    RunSummary {
        outcome,
        stages,
        cancelled_before_mutation: false,
    }
}

fn cleanup_preview(plan: &AggressiveCleanupPlan) -> String {
    let mut lines = vec!["清理预览（基于 fresh 探测）".to_owned()];
    for change in &plan.shell_changes {
        lines.push(format!("  - 更新 shell 配置：{}", change.path.display()));
    }
    for target in &plan.targets {
        lines.push(format!("  - {}：{}", target.label, target.action));
        if !target.affected_packages.is_empty() {
            let impact = if matches!(target.action, model::CleanupAction::ReportOnly) {
                "保留 globals"
            } else {
                "将丢失 globals"
            };
            lines.push(format!(
                "    {impact}：{}",
                target.affected_packages.join(", ")
            ));
        }
        lines.push(format!("    依据：{}", target.evidence));
    }
    for root in &plan.nvm_roots {
        lines.push(format!("  - 删除 nvm data：{}", root.display()));
    }
    for root in &plan.fnm_data_roots {
        lines.push(format!("  - 删除 fnm data：{}", root.display()));
    }
    for root in &plan.fnm_multishell_roots {
        lines.push(format!("  - 删除 fnm multishell：{}", root.display()));
    }
    if plan.cargo_fnm {
        lines.push("  - 执行 cargo uninstall fnm".to_owned());
    }
    for reference in &plan.references {
        lines.push(format!(
            "  - {}旧 runtime 引用：{}：{}",
            reference_impact_label(reference.impact),
            reference.location,
            reference.excerpt
        ));
        lines.push(format!("    依据：{}", reference.evidence));
    }
    if plan.unaffected_reference_count > 0 {
        lines.push(format!(
            "  - 已过滤 {} 条不受 cleanup 影响的引用候选",
            plan.unaffected_reference_count
        ));
    }
    for failed in &plan.global_failures {
        lines.push(format!(
            "  - 警告：{}@{} 未迁移成功；cleanup 后旧副本丢失",
            failed.name, failed.expected_version
        ));
    }
    for package in &plan.unreconstructable_globals {
        let retained_provider = matches!(package.provider.as_str(), "pnpm-home" | "bun")
            || package.provider == "nvm" && plan.nvm_roots.is_empty()
            || package.provider == "fnm" && plan.fnm_data_roots.is_empty();
        let consequence = if retained_provider {
            "对应 provider 保留"
        } else {
            "cleanup 后旧副本丢失"
        };
        lines.push(format!(
            "  - 警告：{}@{} 无法可靠重建（{}）；{consequence}",
            package.name,
            package.version,
            package_source_label(&package.source)
        ));
    }
    for diagnostic in &plan.diagnostics {
        lines.push(format!("  - 阻断：{diagnostic}"));
    }
    if lines.len() == 1 {
        lines.push("  未发现 cleanup action".to_owned());
    }
    lines.join("\n")
}

fn render_globals(stage: &GlobalStage) {
    println!("\nVite+ global 迁移结果");
    for candidate in &stage.inventory.candidates {
        let origins = candidate
            .origins
            .iter()
            .map(|origin| origin.provider.as_str())
            .collect::<Vec<_>>();
        println!(
            "  - {}@{}：{}",
            candidate.name,
            candidate.version,
            origins.join(", ")
        );
    }
    for package in &stage.inventory.unreconstructable {
        println!(
            "  - 仅报告，无法可靠重建：{}@{} ({})",
            package.name,
            package.version,
            package_source_label(&package.source)
        );
    }
    for result in &stage.results {
        println!(
            "  - {}@{}：{}{}",
            result.name,
            result.expected_version,
            global_status_label(result.status),
            result
                .detail
                .as_ref()
                .map(|detail| format!(" ({detail})"))
                .unwrap_or_default()
        );
    }
}

fn print_final(summary: &RunSummary) {
    if summary.cancelled_before_mutation {
        return;
    }
    println!("\n阶段结果");
    for stage in &summary.stages {
        println!(
            "  - {}：{}；{}",
            stage.name,
            stage_status_label(stage.status),
            stage.detail
        );
    }
    for line in &summary.outcome.completed {
        println!("完成：{line}");
    }
    for line in &summary.outcome.failures {
        eprintln!("未完成：{line}");
    }
    if summary.outcome.incomplete {
        eprintln!("结果：部分完成（新安装不会因后续清理失败而回滚）。");
    } else {
        println!("结果：完成。");
    }
}

fn stage_status_label(status: StageStatus) -> &'static str {
    match status {
        StageStatus::Completed => "完成",
        StageStatus::Failed => "失败",
        StageStatus::Skipped => "未执行",
        StageStatus::Partial => "部分完成",
    }
}

fn reference_impact_label(impact: ReferenceImpact) -> &'static str {
    match impact {
        ReferenceImpact::Affected => "将受影响的",
        ReferenceImpact::Uncertain => "影响待确认的",
    }
}

fn global_status_label(status: GlobalStatus) -> &'static str {
    match status {
        GlobalStatus::Installed => "已迁移",
        GlobalStatus::Failed => "失败",
    }
}

fn package_source_label(source: &PackageSource) -> &'static str {
    match source {
        PackageSource::Registry => "registry",
        PackageSource::Git => "git",
        PackageSource::File => "本地 file",
        PackageSource::Link => "本地 link",
        PackageSource::Workspace => "workspace",
        PackageSource::Unknown => "来源未知",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use tempfile::tempdir;

    use super::{cleanup_preview, cleanup_revalidation_passed, run};
    use crate::node::{
        aggressive::AggressiveCleanupPlan,
        cli::Prompter,
        command::{CommandResult, CommandSpec, Runner},
        context::AppContext,
        error::Result,
        model::{
            CleanupAction, CleanupTarget, GlobalPackage, PackageSource, ReferenceFact,
            ReferenceImpact,
        },
        platform::HomePaths,
    };

    #[test]
    fn first_cancel_runs_no_external_command() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let runner = NeverRunner::default();
        let mut prompt = CancelPrompt;
        let mut context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home,
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([(OsString::from("PATH"), OsString::new())]),
        };

        let summary = run(&mut context).unwrap();

        assert!(summary.cancelled_before_mutation);
        assert_eq!(runner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unsafe_zdotdir_fails_before_prompt_or_command() {
        let root = tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let runner = NeverRunner::default();
        let mut prompt = PanicPrompt;
        let mut context = AppContext {
            runner: &runner,
            prompt: &mut prompt,
            home: HomePaths {
                home,
                temp_root: root.path().to_path_buf(),
            },
            environment: BTreeMap::from([(
                OsString::from("ZDOTDIR"),
                OsString::from("/outside/home"),
            )]),
        };

        assert!(run(&mut context).is_err());
        assert_eq!(runner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cleanup_preview_lists_every_executable_action_class() {
        let plan = AggressiveCleanupPlan {
            targets: vec![
                CleanupTarget {
                    label: "homebrew node".to_owned(),
                    action: CleanupAction::RemoveHomebrewFormula("node".to_owned()),
                    evidence: "brew inventory".to_owned(),
                    affected_packages: vec!["old-global".to_owned()],
                },
                CleanupTarget {
                    label: "Bun global eslint".to_owned(),
                    action: CleanupAction::ReportOnly,
                    evidence: "Bun target retained".to_owned(),
                    affected_packages: vec!["eslint".to_owned()],
                },
            ],
            shell_changes: vec![crate::node::cleanup::ShellCleanup {
                path: PathBuf::from("/home/me/.zshrc"),
                expected: b"old".to_vec(),
                content: b"new".to_vec(),
            }],
            nvm_roots: vec![PathBuf::from("/home/me/.nvm")],
            fnm_data_roots: vec![PathBuf::from("/home/me/.local/share/fnm")],
            fnm_multishell_roots: vec![PathBuf::from("/home/me/.cache/fnm_multishells")],
            cargo_fnm: true,
            references: vec![ReferenceFact {
                source: "shell".to_owned(),
                location: "/home/me/.zshrc".to_owned(),
                excerpt: "nvm use".to_owned(),
                impact: ReferenceImpact::Affected,
                evidence: "nvm cleanup".to_owned(),
            }],
            unaffected_reference_count: 0,
            global_failures: Vec::new(),
            unreconstructable_globals: vec![GlobalPackage {
                name: "linked-tool".to_owned(),
                version: "1.0.0".to_owned(),
                source: PackageSource::Link,
                provider: "pnpm-home".to_owned(),
                node_version: None,
                bins: Vec::new(),
            }],
            global_fingerprint: vec!["pnpm-home linked-tool 1.0.0".to_owned()],
            runtime_fingerprint: vec!["nvm 20.11.0".to_owned()],
            diagnostics: vec!["inventory incomplete".to_owned()],
        };

        let preview = cleanup_preview(&plan);

        for expected in [
            ".zshrc",
            "brew uninstall node",
            ".nvm",
            ".local/share/fnm",
            "fnm_multishells",
            "cargo uninstall fnm",
            "Bun global eslint",
            "仅报告，不删除",
            "保留 globals",
            "linked-tool@1.0.0",
            "inventory incomplete",
        ] {
            assert!(
                preview.contains(expected),
                "missing preview item: {expected}"
            );
        }

        let mut changed = plan.clone();
        changed.references.clear();
        assert!(plan.same_actions(&changed));
        changed.global_fingerprint.push("new global".to_owned());
        assert!(!plan.same_actions(&changed));
        let mut runtime_changed = plan.clone();
        runtime_changed
            .runtime_fingerprint
            .push("nvm 22.21.0".to_owned());
        assert!(!plan.same_actions(&runtime_changed));

        let mut clean = plan.clone();
        clean.diagnostics.clear();
        assert!(cleanup_revalidation_passed(&clean, &clean));
        let mut incomplete = clean.clone();
        incomplete.diagnostics.push("read failed".to_owned());
        assert!(!cleanup_revalidation_passed(&clean, &incomplete));
        let mut drifted = clean.clone();
        drifted.global_fingerprint.push("new global".to_owned());
        assert!(!cleanup_revalidation_passed(&clean, &drifted));
    }

    #[derive(Default)]
    struct NeverRunner {
        calls: AtomicUsize,
    }

    impl Runner for NeverRunner {
        fn run(&self, _: &CommandSpec) -> Result<CommandResult> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            panic!("external command must not run")
        }
    }

    struct CancelPrompt;

    impl Prompter for CancelPrompt {
        fn confirm(&mut self, _: &str) -> Result<bool> {
            Ok(false)
        }
    }

    struct PanicPrompt;

    impl Prompter for PanicPrompt {
        fn confirm(&mut self, _: &str) -> Result<bool> {
            panic!("prompt must not run")
        }
    }
}
