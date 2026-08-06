mod cli;
mod icon;
mod node;
mod release;
mod upgrade;
mod zed;

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
jt — personal CLI for Jacob and Taotao

Usage:
  jt repo cicd
  jt node init
  jt cli bootstrap
  jt ghostty install
  jt zed-conf
  jt upgrade [version] [options]
  jt icon <size|svg> [directory]

Commands:
  repo cicd                    Configure npm release automation in current directory
  node init                    Initialize global Node/pnpm environment with Vite+
  cli bootstrap                Bootstrap shell, shortcuts, prompt, and CLI tools
  ghostty install              Install and configure Ghostty on macOS
  zed-conf                     Write live Zed config to current Git repository
  upgrade [version]            Upgrade jt from a published GitHub Release
  icon <size|svg> [directory]  Write one JT icon; default directory: ./public

Options:
  -h, --help                   Print help
  -V, --version                Print version

PNG sizes:
  16, 24, 32, 48, 64, 128, 256, 512, 1024
";

fn main() -> ExitCode {
    let args = env::args_os().skip(1).collect::<Vec<_>>();

    if matches!(args.as_slice(), [arg] if is(arg, "-h") || is(arg, "--help")) {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if matches!(args.as_slice(), [arg] if is(arg, "-V") || is(arg, "--version")) {
        println!("jt {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    match args.as_slice() {
        [command, action] if is(command, "repo") && is(action, "cicd") => repo_cicd(),
        [command, action] if is(command, "node") && is(action, "init") => node_init(),
        [command, action] if is(command, "cli") && is(action, "bootstrap") => cli_bootstrap(),
        [command, action] if is(command, "ghostty") && is(action, "install") => ghostty_install(),
        [command] if is(command, "zed-conf") => ExitCode::from(zed::run()),
        [command, upgrade_args @ ..] if is(command, "upgrade") => {
            ExitCode::from(upgrade::run(upgrade_args))
        }
        [command, selector] if is(command, "icon") => icon_download(selector, None),
        [command, selector, directory] if is(command, "icon") => {
            icon_download(selector, Some(directory))
        }
        _ => {
            eprint!("{HELP}");
            ExitCode::from(2)
        }
    }
}

fn repo_cicd() -> ExitCode {
    let result = env::current_dir()
        .map_err(|error| format!("cannot read current directory: {error}"))
        .and_then(|directory| release::init(&directory));

    match result {
        Ok(release::InitStatus::Created(path)) => {
            println!("created {}", path.display());
            ExitCode::SUCCESS
        }
        Ok(release::InitStatus::Unchanged(path)) => {
            println!("already configured {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn node_init() -> ExitCode {
    ExitCode::from(node::init())
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

fn is(argument: &OsString, expected: &str) -> bool {
    argument == OsStr::new(expected)
}
