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
    #[command(name = "generate", about = "Run complete frontend API generation")]
    Generate(nlab_api::GenerateArgs),
    #[command(name = "config", about = "Configure the project-local nlab-api runner")]
    Config(nlab_api::ConfigArgs),
    #[command(name = "update", about = "Check and update nlab-api")]
    Update(update::UpdateArgs),
}

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let cli = Cli::parse();
    if !cli.no_update && !matches!(&cli.command, Command::Config(_) | Command::Update(_)) {
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
        Command::Config(args) => nlab_api::configure(args),
        Command::Update(args) => update::run(args),
    };
    ExitCode::from(status)
}
