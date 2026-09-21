#[path = "../update.rs"]
mod update;

use std::env;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "nlab-api",
    version = env!("CARGO_PKG_VERSION"),
    about = "Generate frontend contracts from NLab Java APIs",
    subcommand_required = true,
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
struct Cli {
    /// Skip automatic update checks
    #[arg(long, global = true, hide = true)]
    no_update: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
#[command(disable_help_subcommand = true)]
enum Command {
    #[command(
        name = "init",
        about = "Inspect and configure a frontend project for nlab-api generation"
    )]
    Init(nlab_api::InitArgs),
    #[command(
        name = "generate",
        about = "Generate frontend APIs with automatic cross-repository discovery",
        after_long_help = "Only code-verified enum primary values generate enum definitions. Unverified comment candidates and auxiliary properties remain scalar."
    )]
    Generate(nlab_api::GenerateArgs),
    #[command(
        name = "discover",
        about = "Discover backend services and refresh their source repositories"
    )]
    Discover(nlab_api::DiscoverArgs),
    #[command(
        name = "mock",
        about = "Generate semantic mock JSON and native Whistle rules"
    )]
    Mock(nlab_api::MockArgs),
    #[command(name = "config", about = "Configure the project-local nlab-api runner")]
    Config(nlab_api::ConfigArgs),
    #[command(
        name = "update",
        visible_alias = "upgrade",
        about = "Update nlab-api and synchronize its installed Skill through Skill Manager"
    )]
    Update(update::UpdateArgs),
}

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let cli = Cli::parse();
    if !cli.no_update
        && !matches!(
            &cli.command,
            Command::Config(_) | Command::Update(_) | Command::Discover(_)
        )
    {
        match update::auto_update(&arguments) {
            Ok(Some(status)) => return status,
            Ok(None) => {}
            Err(error) => {
                eprintln!("error: automatic nlab-api update failed: {error:#}");
                return ExitCode::FAILURE;
            }
        }
    }

    let status = match cli.command {
        Command::Init(args) => nlab_api::init(args),
        Command::Generate(args) => nlab_api::generate(args),
        Command::Discover(args) => nlab_api::discover(args),
        Command::Mock(args) => nlab_api::mock(args),
        Command::Config(args) => nlab_api::configure(args),
        Command::Update(args) => update::run(args),
    };
    ExitCode::from(status)
}
