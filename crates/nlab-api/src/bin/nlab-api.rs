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
}

fn main() -> ExitCode {
    let status = match Cli::parse().command {
        Command::Init(args) => nlab_api::init(args),
        Command::Generate(args) => nlab_api::generate(args),
    };
    ExitCode::from(status)
}
