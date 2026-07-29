mod release;

use std::env;
use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

const HELP: &str = "\
jt — personal CLI for Jacob and Taotao

Usage:
  jt release init

Commands:
  release init    Configure npm release automation in current directory
";

fn main() -> ExitCode {
    let args = env::args_os().skip(1).collect::<Vec<_>>();

    if matches!(args.as_slice(), [arg] if is(arg, "-h") || is(arg, "--help")) {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }

    if !matches!(args.as_slice(), [command, action] if is(command, "release") && is(action, "init"))
    {
        eprint!("{HELP}");
        return ExitCode::from(2);
    }

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

fn is(argument: &OsString, expected: &str) -> bool {
    argument == OsStr::new(expected)
}
