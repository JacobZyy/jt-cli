mod ai_hook;
mod cli;
mod codex;
mod icon;
mod node;
mod release;
mod unused;
mod upgrade;
mod zed;

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use semver::Version;

#[derive(Debug, Parser)]
#[command(
    name = "jt",
    version = env!("CARGO_PKG_VERSION"),
    about = "jt — personal CLI for Jacob and Taotao",
    subcommand_required = true,
    arg_required_else_help = true,
    disable_help_subcommand = true,
    after_help = "\
Examples:
  jt repo cicd
  jt node init
  jt codex init
  jt cli bootstrap
  jt ghostty install
  jt zed-conf
  jt ai-hook
  jt ai-hook --checks vitest,eslint --agents codex
  jt code unused [PATH]
  jt unused [PATH]
  jt call-graph [PATH]
  jt vitest
  jt upgrade [version] [options]
  jt icon <size|svg> [directory]

PNG sizes:
  16, 24, 32, 48, 64, 128, 256, 512, 1024
"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum Commands {
    #[command(about = "Configure project-local Codex environments")]
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    #[command(name = "repo", about = "Configure Node.js/Rust release automation")]
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    #[command(name = "node", about = "Initialize global Node/pnpm environment")]
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    #[command(
        name = "cli",
        about = "Bootstrap shell, shortcuts, prompt, and CLI tools"
    )]
    Cli {
        #[command(subcommand)]
        command: CliCommand,
    },
    #[command(name = "ghostty", about = "Install and configure Ghostty on macOS")]
    Ghostty {
        #[command(subcommand)]
        command: GhosttyCommand,
    },
    #[command(
        name = "zed-conf",
        about = "Write live Zed config to current Git repository"
    )]
    ZedConf,
    #[command(name = "ai-hook", about = "Configure project AI hooks")]
    AiHook(ai_hook::AiHookArgs),
    #[command(name = "code", about = "Analyze project source code")]
    Code {
        #[command(subcommand)]
        command: CodeCommand,
    },
    #[command(
        name = "unused",
        about = "Find unused JavaScript, TypeScript, and Vue code"
    )]
    Unused(unused::UnusedArgs),
    #[command(
        name = "call-graph",
        about = "Write an interactive JavaScript, TypeScript, and Vue call graph"
    )]
    CallGraph(unused::CallGraphArgs),
    #[command(name = "vitest", about = "Vitest automation (not implemented yet)")]
    Vitest,
    #[command(name = "upgrade", about = "Upgrade jt from a published GitHub Release")]
    Upgrade(UpgradeArgs),
    #[command(name = "icon", about = "Write one JT icon")]
    Icon(IconArgs),
    #[command(name = "completions", about = "Generate shell completion script")]
    Completions(CompletionsArgs),
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum RepoCommand {
    #[command(
        name = "cicd",
        about = "Configure release automation in current directory"
    )]
    Cicd,
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum CodexCommand {
    #[command(about = "Create environment.toml in the current project without running setup")]
    Init {
        #[arg(
            long,
            conflicts_with = "update",
            help = "Write a candidate for review without replacing existing configuration"
        )]
        check: bool,
        #[arg(
            long,
            conflicts_with = "check",
            help = "Update an unchanged managed template, preserving a recovery copy"
        )]
        update: bool,
        #[arg(
            long,
            help = "Reduce Cargo debug and incremental output in generated build/test actions"
        )]
        lightweight: bool,
    },
    #[command(
        about = "Inventory worktrees or check completion evidence before native Codex archival"
    )]
    Worktrees {
        #[arg(long, value_name = "EVIDENCE_JSON")]
        check: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum NodeCommand {
    #[command(
        name = "init",
        about = "Initialize global Node/pnpm environment with Vite+"
    )]
    Init {
        /// npm registry URL (independent of the Node binary download mirror).
        #[arg(long, value_name = "URL", default_value = "https://registry.npmmirror.com/", value_parser = node::parse_npm_registry)]
        npmregister: String,
    },
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum CliCommand {
    #[command(
        name = "bootstrap",
        about = "Bootstrap shell, shortcuts, prompt, and CLI tools"
    )]
    Bootstrap,
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum CodeCommand {
    #[command(
        name = "unused",
        about = "Find unused JavaScript, TypeScript, and Vue code"
    )]
    Unused(unused::UnusedArgs),
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum GhosttyCommand {
    #[command(name = "install", about = "Install and configure Ghostty on macOS")]
    Install,
}

#[derive(Debug, Args)]
struct UpgradeArgs {
    /// Exact SemVer to install, with optional v prefix; default: latest
    #[arg(value_name = "version", value_parser = upgrade::parse_version)]
    version: Option<Version>,
    /// Check without installing
    #[arg(long, conflicts_with = "dry_run")]
    check: bool,
    /// Print the Cargo command without installing
    #[arg(long, conflicts_with = "check")]
    dry_run: bool,
    /// Reinstall when target equals current version
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct IconArgs {
    /// PNG size, svg, or animated
    #[arg(value_name = "size|svg|animated")]
    selector: OsString,
    /// Output directory; default: ./public
    #[arg(value_name = "directory")]
    directory: Option<OsString>,
}

#[derive(Debug, Args)]
struct CompletionsArgs {
    #[arg(value_enum)]
    shell: CompletionShell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CompletionShell {
    Fish,
    Zsh,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Codex {
            command:
                CodexCommand::Init {
                    check,
                    update,
                    lightweight,
                },
        } => ExitCode::from(codex::run(check, update, lightweight)),
        Commands::Codex {
            command: CodexCommand::Worktrees { check },
        } => ExitCode::from(codex::recycle::run(check.as_deref())),
        Commands::Repo {
            command: RepoCommand::Cicd,
        } => repo_cicd(),
        Commands::Node {
            command: NodeCommand::Init { npmregister },
        } => node_init(&npmregister),
        Commands::Cli {
            command: CliCommand::Bootstrap,
        } => cli_bootstrap(),
        Commands::Ghostty {
            command: GhosttyCommand::Install,
        } => ghostty_install(),
        Commands::ZedConf => ExitCode::from(zed::run()),
        Commands::AiHook(args) => ExitCode::from(ai_hook::run(args)),
        Commands::Code {
            command: CodeCommand::Unused(args),
        } => ExitCode::from(unused::run(args)),
        Commands::Unused(args) => ExitCode::from(unused::run(args)),
        Commands::CallGraph(args) => ExitCode::from(unused::run_call_graph(args)),
        Commands::Vitest => {
            println!("Vitest functionality is not implemented yet.");
            ExitCode::SUCCESS
        }
        Commands::Upgrade(args) => upgrade(args),
        Commands::Icon(args) => icon_download(&args.selector, args.directory.as_ref()),
        Commands::Completions(args) => completions(args.shell),
    }
}

fn upgrade(args: UpgradeArgs) -> ExitCode {
    let mut arguments = Vec::with_capacity(4);
    if let Some(version) = args.version {
        arguments.push(OsString::from(version.to_string()));
    }
    if args.check {
        arguments.push(OsString::from("--check"));
    }
    if args.dry_run {
        arguments.push(OsString::from("--dry-run"));
    }
    if args.force {
        arguments.push(OsString::from("--force"));
    }
    ExitCode::from(upgrade::run(&arguments))
}

fn completions(shell: CompletionShell) -> ExitCode {
    let mut command = Cli::command();
    let shell = match shell {
        CompletionShell::Fish => clap_complete::Shell::Fish,
        CompletionShell::Zsh => clap_complete::Shell::Zsh,
    };
    clap_complete::generate(shell, &mut command, "jt", &mut std::io::stdout());
    ExitCode::SUCCESS
}

fn repo_cicd() -> ExitCode {
    let result = env::current_dir()
        .map_err(|error| format!("cannot read current directory: {error}"))
        .and_then(|directory| release::init(&directory));

    match result {
        Ok(release::InitStatus::Created(paths)) => {
            for path in paths {
                println!("created {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Ok(release::InitStatus::Unchanged(paths)) => {
            for path in paths {
                println!("already configured {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn node_init(npmregister: &str) -> ExitCode {
    ExitCode::from(node::init(npmregister))
}

fn cli_bootstrap() -> ExitCode {
    ExitCode::from(cli::bootstrap())
}

fn ghostty_install() -> ExitCode {
    ExitCode::from(cli::ghostty_install())
}

fn icon_download(selector: &OsString, output_directory: Option<&OsString>) -> ExitCode {
    let result = env::current_dir()
        .map_err(|error| format!("cannot read current directory: {error}"))
        .and_then(|current_directory| {
            let directory = output_directory
                .map(PathBuf::from)
                .map(|path| {
                    if path.is_absolute() {
                        path
                    } else {
                        current_directory.join(path)
                    }
                })
                .unwrap_or_else(|| current_directory.join("public"));
            icon::download(selector, &directory)
        });

    match result {
        Ok(path) => {
            println!("created {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod node_registry_tests {
    use super::*;

    #[test]
    fn node_registry_cli_defaults_and_normalizes() {
        for (args, expected) in [
            (
                vec!["jt", "node", "init"],
                "https://registry.npmmirror.com/",
            ),
            (
                vec![
                    "jt",
                    "node",
                    "init",
                    "--npmregister",
                    "https://registry.npmjs.org",
                ],
                "https://registry.npmjs.org/",
            ),
            (
                vec![
                    "jt",
                    "node",
                    "init",
                    "--npmregister",
                    "https://packages.example/npm",
                ],
                "https://packages.example/npm/",
            ),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Commands::Node {
                command: NodeCommand::Init { npmregister },
            } = cli.command
            else {
                panic!("expected node init")
            };
            assert_eq!(npmregister, expected);
        }
    }

    #[test]
    fn node_registry_cli_rejects_invalid_urls_before_execution() {
        for value in [
            "",
            "registry.npmjs.org",
            "https:foo",
            "file:///tmp/npm",
            "https://",
            "https://user:pass@example.com",
            "https://example.com?q=1",
            "https://example.com/#fragment",
            "https://example.com/a b",
            "https://example.com\\evil",
        ] {
            assert!(
                Cli::try_parse_from(["jt", "node", "init", "--npmregister", value]).is_err(),
                "accepted {value:?}"
            );
        }
        assert!(Cli::try_parse_from(["jt", "node", "init", "--npmregister"]).is_err());
    }
}
