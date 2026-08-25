use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use semver::Version;

use crate::node::error::{AppError, Result};
use crate::node::fs::{atomic_write, read_optional};
use crate::node::platform::{HomePaths, first_executable, supported_platform};

const REPOSITORY_URL: &str = "https://github.com/JacobZyy/jt-cli";
const HELP: &str = "\
Upgrade jt from a published GitHub Release

Usage:
  jt upgrade [version] [options]

Arguments:
  [version]  Exact SemVer to install, with optional v prefix; default: latest

Options:
  --check    Check without installing
  --dry-run  Print the Cargo command without installing
  --force    Reinstall when target equals current version
  -h, --help Print help
";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Options {
    version: Option<Version>,
    check: bool,
    dry_run: bool,
    force: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ParsedOptions {
    Help,
    Run(Options),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Release {
    tag: String,
    version: Version,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstallPlan {
    program: PathBuf,
    args: Vec<OsString>,
}

pub fn run(arguments: &[OsString]) -> u8 {
    let parsed = match parse_options(arguments) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    let ParsedOptions::Run(options) = parsed else {
        print!("{HELP}");
        return 0;
    };

    match execute(&options) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn execute(options: &Options) -> Result<()> {
    supported_platform()?;
    let environment = env::vars_os().collect::<BTreeMap<_, _>>();
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| AppError::Invalid(format!("invalid embedded jt version: {error}")))?;
    let release = fetch_release(options.version.as_ref(), &environment)?;
    let ordering = release.version.cmp(&current);

    if options.check {
        print_check_result(&current, &release.version, ordering);
        return Ok(());
    }
    if ordering == Ordering::Equal && !options.force {
        println!("jt is already up to date ({current})");
        return Ok(());
    }
    if ordering == Ordering::Less && options.version.is_none() {
        println!(
            "installed jt {current} is newer than latest published release {}",
            release.version
        );
        return Ok(());
    }

    let home = HomePaths::from_environment(&environment)?.home;
    let executable = env::current_exe()
        .map_err(|error| AppError::io("resolve current jt executable", None, error))?;
    validate_install_target(&home, &executable)?;
    let cargo = first_executable("cargo", &environment).unwrap_or_else(|| PathBuf::from("cargo"));
    let revision = resolve_tag_revision(&release.tag, &environment)?;

    let action = match ordering {
        Ordering::Greater => "Upgrading",
        Ordering::Equal => "Reinstalling",
        Ordering::Less => "Downgrading",
    };
    println!("{action} jt {current} -> {}", release.version);

    if options.dry_run {
        let plan = build_install_plan(&cargo, &revision, Path::new("<temporary-directory>"));
        println!("Run: {}", display_command(&plan));
        println!("Dry run: no changes made.");
        return Ok(());
    }
    if !cargo.is_file() {
        return Err(AppError::Invalid(
            "cargo is required; install Rust from https://rustup.rs".to_owned(),
        ));
    }

    let staging = tempfile::tempdir()
        .map_err(|error| AppError::io("create upgrade staging directory", None, error))?;
    let plan = build_install_plan(&cargo, &revision, staging.path());
    println!("Run: {}", display_command(&plan));
    run_install(&plan)?;

    let staged = staging.path().join("bin/jt");
    verify_binary(&staged, &release.version, "verify staged jt")?;
    replace_binary(&home, &executable, &staged, &release.version)?;

    println!("Updated jt to {}", release.version);
    println!("Binary: {}", executable.display());
    println!(
        "Release notes: {REPOSITORY_URL}/releases/tag/{}",
        release.tag
    );
    Ok(())
}

fn parse_options(arguments: &[OsString]) -> Result<ParsedOptions> {
    let mut options = Options::default();
    for argument in arguments {
        let argument = argument
            .to_str()
            .ok_or_else(|| AppError::Invalid("upgrade arguments must be valid UTF-8".to_owned()))?;
        match argument {
            "-h" | "--help" => return Ok(ParsedOptions::Help),
            "--check" => options.check = true,
            "--dry-run" => options.dry_run = true,
            "--force" => options.force = true,
            value if value.starts_with('-') => {
                return Err(AppError::Invalid(format!(
                    "unknown upgrade option: {value}"
                )));
            }
            value => {
                if options.version.is_some() {
                    return Err(AppError::Invalid(
                        "upgrade accepts at most one version".to_owned(),
                    ));
                }
                options.version = Some(parse_version(value)?);
            }
        }
    }
    if options.check && options.dry_run {
        return Err(AppError::Invalid(
            "--check and --dry-run cannot be combined".to_owned(),
        ));
    }
    Ok(ParsedOptions::Run(options))
}

pub(crate) fn parse_version(value: &str) -> Result<Version> {
    let value = value.strip_prefix('v').unwrap_or(value);
    Version::parse(value).map_err(|error| AppError::Invalid(format!("invalid jt version: {error}")))
}

fn fetch_release(
    requested: Option<&Version>,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Release> {
    let curl = first_executable("curl", environment)
        .ok_or_else(|| AppError::Invalid("curl is required to query GitHub Releases".to_owned()))?;
    let url = requested.map_or_else(
        || format!("{REPOSITORY_URL}/releases/latest"),
        |version| format!("{REPOSITORY_URL}/releases/tag/v{version}"),
    );
    let output = Command::new(&curl)
        .args([
            "-fsSIL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--retry",
            "3",
            "--max-time",
            "30",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            &url,
        ])
        .output()
        .map_err(|error| AppError::io("query GitHub Release", Some(curl), error))?;
    if !output.status.success() {
        return Err(AppError::Command {
            action: "query GitHub Release".to_owned(),
            status: output.status.code().unwrap_or(1),
            detail: last_nonempty_line(&output.stderr),
        });
    }
    let release = parse_release_url(&String::from_utf8_lossy(&output.stdout))?;
    if requested.is_some_and(|version| version != &release.version) {
        return Err(AppError::Invalid(format!(
            "GitHub returned unexpected release {}",
            release.tag
        )));
    }
    Ok(release)
}

fn parse_release_url(url: &str) -> Result<Release> {
    let prefix = format!("{REPOSITORY_URL}/releases/tag/");
    let tag = url.trim().strip_prefix(&prefix).ok_or_else(|| {
        AppError::Invalid(format!(
            "GitHub latest release resolved outside jt releases: {}",
            url.trim()
        ))
    })?;
    let version = parse_version(tag)?;
    if tag != format!("v{version}") {
        return Err(AppError::Invalid(format!(
            "GitHub Release tag must use v<semver>: {tag}"
        )));
    }
    Ok(Release {
        tag: tag.to_owned(),
        version,
    })
}

fn print_check_result(current: &Version, target: &Version, ordering: Ordering) {
    match ordering {
        Ordering::Greater => println!("Update available: {current} -> {target}"),
        Ordering::Equal => println!("jt is already up to date ({current})"),
        Ordering::Less => println!("Target {target} is older than installed jt {current}"),
    }
}

fn validate_install_target(home: &Path, executable: &Path) -> Result<()> {
    let supported = home.join(".local/bin/jt");
    if executable != supported {
        return Err(AppError::UnsafePath(format!(
            "jt upgrade supports only {}; current executable: {}. Use the owning installer or package manager",
            supported.display(),
            executable.display(),
        )));
    }
    let parent = executable.parent().expect("validated parent");
    let resolved_parent = parent.canonicalize().map_err(|error| {
        AppError::io(
            "resolve jt install directory",
            Some(parent.to_path_buf()),
            error,
        )
    })?;
    if !resolved_parent.starts_with(home) {
        return Err(AppError::UnsafePath(format!(
            "jt executable is outside HOME; use its owning package manager: {}",
            executable.display()
        )));
    }
    let metadata = fs::symlink_metadata(executable).map_err(|error| {
        AppError::io(
            "inspect current jt executable",
            Some(executable.to_path_buf()),
            error,
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AppError::UnsafePath(format!(
            "jt upgrade refuses a non-regular executable: {}",
            executable.display()
        )));
    }
    Ok(())
}

fn resolve_tag_revision(tag: &str, environment: &BTreeMap<OsString, OsString>) -> Result<String> {
    let git = first_executable("git", environment).ok_or_else(|| {
        AppError::Invalid("git is required to resolve the release tag".to_owned())
    })?;
    let direct = format!("refs/tags/{tag}");
    let peeled = format!("{direct}^{{}}");
    let output = Command::new(&git)
        .args(["ls-remote", "--exit-code", REPOSITORY_URL, &direct, &peeled])
        .output()
        .map_err(|error| AppError::io("resolve GitHub release tag", Some(git), error))?;
    if !output.status.success() {
        return Err(AppError::Command {
            action: "resolve GitHub release tag".to_owned(),
            status: output.status.code().unwrap_or(1),
            detail: last_nonempty_line(&output.stderr),
        });
    }
    parse_tag_revision(&output.stdout, &direct, &peeled)
}

fn parse_tag_revision(content: &[u8], direct: &str, peeled: &str) -> Result<String> {
    let mut direct_revision = None;
    let mut peeled_revision = None;
    for line in String::from_utf8_lossy(content).lines() {
        let mut fields = line.split_whitespace();
        let revision = fields.next().unwrap_or("");
        let reference = fields.next().unwrap_or("");
        if fields.next().is_some()
            || !matches!(revision.len(), 40 | 64)
            || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AppError::Invalid(
                "git returned an invalid release tag revision".to_owned(),
            ));
        }
        if reference == direct {
            direct_revision = Some(revision.to_owned());
        } else if reference == peeled {
            peeled_revision = Some(revision.to_owned());
        } else {
            return Err(AppError::Invalid(format!(
                "git returned an unexpected release tag reference: {reference}"
            )));
        }
    }
    peeled_revision
        .or(direct_revision)
        .ok_or_else(|| AppError::Invalid("GitHub release tag has no Git revision".to_owned()))
}

fn build_install_plan(cargo: &Path, revision: &str, root: &Path) -> InstallPlan {
    InstallPlan {
        program: cargo.to_path_buf(),
        args: vec![
            OsString::from("install"),
            OsString::from("--git"),
            OsString::from(REPOSITORY_URL),
            OsString::from("--rev"),
            OsString::from(revision),
            OsString::from("--locked"),
            OsString::from("--root"),
            root.as_os_str().to_os_string(),
            OsString::from("jt"),
        ],
    }
}

fn display_command(plan: &InstallPlan) -> String {
    std::iter::once(plan.program.as_os_str())
        .chain(plan.args.iter().map(OsString::as_os_str))
        .map(shell_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_word(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/._-:=+@".contains(character))
    {
        value.into_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn run_install(plan: &InstallPlan) -> Result<()> {
    let status = Command::new(&plan.program)
        .args(&plan.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| AppError::io("start cargo upgrade", Some(plan.program.clone()), error))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::Command {
            action: "cargo install jt".to_owned(),
            status: status.code().unwrap_or(1),
            detail: "Cargo output shown above; current jt was not changed".to_owned(),
        })
    }
}

fn verify_binary(executable: &Path, expected: &Version, action: &str) -> Result<()> {
    let output = Command::new(executable)
        .arg("--version")
        .output()
        .map_err(|error| AppError::io(action, Some(executable.to_path_buf()), error))?;
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let expected = format!("jt {expected}");
    if output.status.success() && actual == expected {
        Ok(())
    } else {
        Err(AppError::Invalid(format!(
            "{action} failed: expected {expected:?}, got {actual:?}"
        )))
    }
}

fn replace_binary(home: &Path, executable: &Path, staged: &Path, version: &Version) -> Result<()> {
    let previous = read_optional(executable)?.ok_or_else(|| {
        AppError::Invalid(format!("current jt disappeared: {}", executable.display()))
    })?;
    let replacement = fs::read(staged)
        .map_err(|error| AppError::io("read staged jt", Some(staged.to_path_buf()), error))?;
    atomic_write(home, executable, Some(&previous), &replacement)?;

    if let Err(verification) = verify_binary(executable, version, "verify installed jt") {
        return match atomic_write(home, executable, Some(&replacement), &previous) {
            Ok(()) => Err(AppError::Invalid(format!(
                "{verification}; restored previous jt"
            ))),
            Err(rollback) => Err(AppError::Invalid(format!(
                "{verification}; rollback failed: {rollback}"
            ))),
        };
    }
    Ok(())
}

fn last_nonempty_line(content: &[u8]) -> String {
    String::from_utf8_lossy(content)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("command returned non-zero")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use semver::Version;
    use tempfile::tempdir;

    use super::{
        Options, ParsedOptions, build_install_plan, display_command, parse_options,
        parse_release_url, parse_tag_revision, replace_binary, validate_install_target,
    };

    #[test]
    fn parses_upgrade_options_and_rejects_shell_shaped_version() {
        assert_eq!(
            parse_options(&[
                OsString::from("v1.2.3"),
                OsString::from("--dry-run"),
                OsString::from("--force"),
            ])
            .unwrap(),
            ParsedOptions::Run(Options {
                version: Some(Version::parse("1.2.3").unwrap()),
                dry_run: true,
                force: true,
                ..Options::default()
            })
        );
        assert!(parse_options(&[OsString::from("1.2.3 && rm -rf /")]).is_err());
        assert!(parse_options(&[OsString::from("--check"), OsString::from("--dry-run")]).is_err());
    }

    #[test]
    fn parses_only_canonical_release_tags() {
        let release =
            parse_release_url("https://github.com/JacobZyy/jt-cli/releases/tag/v1.2.3").unwrap();
        assert_eq!(release.version, Version::parse("1.2.3").unwrap());
        assert!(
            parse_release_url("https://github.com/JacobZyy/jt-cli/releases/tag/release-1.2.3")
                .is_err()
        );
        assert!(parse_release_url("https://example.com/releases/tag/v1.2.3").is_err());
    }

    #[test]
    fn builds_shell_free_cargo_plan_and_quotes_display_only() {
        let plan = build_install_plan(
            Path::new("/path with spaces/cargo"),
            "0123456789abcdef0123456789abcdef01234567",
            Path::new("/tmp/stage root"),
        );

        assert_eq!(
            plan.args[4],
            std::ffi::OsString::from("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(plan.args[7], std::ffi::OsString::from("/tmp/stage root"));
        assert_eq!(
            display_command(&plan),
            "'/path with spaces/cargo' install --git https://github.com/JacobZyy/jt-cli --rev 0123456789abcdef0123456789abcdef01234567 --locked --root '/tmp/stage root' jt"
        );
    }

    #[test]
    fn prefers_peeled_annotated_tag_revision() {
        let direct = "refs/tags/v1.2.3";
        let peeled = "refs/tags/v1.2.3^{}";
        let output = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\trefs/tags/v1.2.3\n\
                       bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\trefs/tags/v1.2.3^{}\n";

        assert_eq!(
            parse_tag_revision(output, direct, peeled).unwrap(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert!(parse_tag_revision(b"bad\trefs/tags/v1.2.3\n", direct, peeled).is_err());
    }

    #[test]
    fn accepts_only_user_owned_bin_installation() {
        let home = tempdir().unwrap();
        let canonical_home = home.path().canonicalize().unwrap();
        let bin = canonical_home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let executable = bin.join("jt");
        std::fs::write(&executable, "binary").unwrap();

        assert!(validate_install_target(&canonical_home, &executable).is_ok());

        let development = canonical_home.join("target/debug");
        std::fs::create_dir_all(&development).unwrap();
        let development_executable = development.join("jt");
        std::fs::write(&development_executable, "binary").unwrap();
        assert!(validate_install_target(&canonical_home, &development_executable).is_err());
        assert!(validate_install_target(&canonical_home, Path::new("/usr/local/bin/jt")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn atomically_replaces_verified_binary_and_rolls_back_bad_replacement() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempdir().unwrap();
        let canonical_home = home.path().canonicalize().unwrap();
        let bin = canonical_home.join("tools/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let executable = bin.join("jt");
        let staged = canonical_home.join("staged-jt");
        let bad = canonical_home.join("bad-jt");
        let old = b"#!/bin/sh\nprintf 'jt 1.1.0\\n'\n";
        let next = b"#!/bin/sh\nprintf 'jt 1.2.0\\n'\n";
        let invalid = b"#!/bin/sh\nprintf 'wrong\\n'\n";

        for (path, content) in [
            (&executable, old.as_slice()),
            (&staged, next),
            (&bad, invalid),
        ] {
            std::fs::write(path, content).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        replace_binary(
            &canonical_home,
            &executable,
            &staged,
            &Version::parse("1.2.0").unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&executable).unwrap(), next);

        assert!(
            replace_binary(
                &canonical_home,
                &executable,
                &bad,
                &Version::parse("1.3.0").unwrap(),
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&executable).unwrap(), next);
    }
}
