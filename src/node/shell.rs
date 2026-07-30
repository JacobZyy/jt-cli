use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use crate::node::{
    error::{AppError, Result},
    model::VITE_PLUS_HOME_DIR,
    platform::{first_executable, value},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
}

pub struct LegacyCleanupScope<'a> {
    pub home: &'a Path,
    pub nvm_roots: &'a [PathBuf],
    pub fnm_roots: &'a [PathBuf],
    pub fnm_multishell_roots: &'a [PathBuf],
    pub pnpm_roots: &'a [PathBuf],
    pub remove_manager_block: bool,
}

const OPEN: &str = "# >>> jt node init vite-plus >>>";
const CLOSE: &str = "# <<< jt node init vite-plus <<<";
const LEGACY_OPEN: &str = "# >>> nlab-node-env-init vite-plus >>>";
const LEGACY_CLOSE: &str = "# <<< nlab-node-env-init vite-plus <<<";
const UPSTREAM_OPEN: &str = "# Vite+ bin (https://viteplus.dev)";
const MANAGER_OPEN: &str = "# >>> nlab-node-env-init node-auto-switch >>>";
const MANAGER_CLOSE: &str = "# <<< nlab-node-env-init node-auto-switch <<<";
const LEGACY_MANAGER_CLOSE: &str = "# <<< nlab-node-env-init <<<";
const LEGACY_MANAGER_OPENS: [&str; 2] = [
    "# >>> nlab-node-env-init fnm >>>",
    "# >>> nlab-node-env-init nvm-auto-switch >>>",
];

pub fn vite_block(shell: ShellKind) -> String {
    match shell {
        ShellKind::Fish => format!(
            "{OPEN}\nset -q VP_HOME; or set -gx VP_HOME \"$HOME/{VITE_PLUS_HOME_DIR}\"\ntest -s \"$VP_HOME/env.fish\"; and source \"$VP_HOME/env.fish\"\n{CLOSE}"
        ),
        ShellKind::Zsh | ShellKind::Bash => format!(
            "{OPEN}\nexport VP_HOME=\"${{VP_HOME:-$HOME/{VITE_PLUS_HOME_DIR}}}\"\n[ -s \"$VP_HOME/env\" ] && . \"$VP_HOME/env\"\n{CLOSE}"
        ),
    }
}

pub fn upsert_vite_block_last(content: &str, shell: ShellKind) -> String {
    let without_block = remove_vite_loaders(content);
    let without = without_block.trim_end();
    let block = vite_block(shell);
    if without.is_empty() {
        format!("{block}\n")
    } else {
        format!("{without}\n\n{block}\n")
    }
}

pub fn reconcile_vite_loader(content: &str, shell: ShellKind, enabled: bool) -> String {
    if enabled {
        upsert_vite_block_last(content, shell)
    } else {
        let next = remove_vite_loaders(content);
        if next.is_empty() {
            next
        } else {
            next.trim_end().to_owned() + "\n"
        }
    }
}

pub fn remove_vite_loaders(content: &str) -> String {
    let without_block = remove_marker_block(content);
    let lines = without_block.lines().collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed == UPSTREAM_OPEN
            && lines
                .get(index + 1)
                .is_some_and(|next| is_vite_source_line(next.trim()))
        {
            index += 2;
            continue;
        }
        if is_vite_source_line(trimmed) {
            index += 1;
            continue;
        }
        result.push(line);
        index += 1;
    }
    result.join("\n")
}

fn is_vite_source_line(line: &str) -> bool {
    matches!(
        line,
        r#". "$HOME/.vite-plus/env""#
            | r#"source "$HOME/.vite-plus/env""#
            | r#". "$VP_HOME/env""#
            | r#"source "$VP_HOME/env""#
            | r#"source "$HOME/.vite-plus/env.fish""#
            | r#"source "$VP_HOME/env.fish""#
    )
}

pub fn remove_marker_block(content: &str) -> String {
    remove_owned_blocks(content, &[(OPEN, CLOSE), (LEGACY_OPEN, LEGACY_CLOSE)])
}

pub fn remove_manager_block(content: &str) -> String {
    remove_owned_blocks(
        content,
        &[
            (MANAGER_OPEN, MANAGER_CLOSE),
            (LEGACY_MANAGER_OPENS[0], LEGACY_MANAGER_CLOSE),
            (LEGACY_MANAGER_OPENS[1], LEGACY_MANAGER_CLOSE),
        ],
    )
}

fn remove_owned_blocks(content: &str, markers: &[(&str, &str)]) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        let Some((_, close)) = markers.iter().find(|(open, _)| trimmed == *open) else {
            result.push(lines[index]);
            index += 1;
            continue;
        };
        let close_offset = lines[index + 1..]
            .iter()
            .position(|line| line.trim() == *close)
            .map(|offset| offset + 1);
        let nested_open_offset = lines[index + 1..]
            .iter()
            .position(|line| markers.iter().any(|(open, _)| line.trim() == *open));
        let Some(close_offset) = close_offset else {
            result.push(lines[index]);
            index += 1;
            continue;
        };
        if nested_open_offset.is_some_and(|nested| nested < close_offset) {
            result.push(lines[index]);
            index += 1;
            continue;
        }
        index += close_offset + 1;
    }
    result.join("\n")
}

fn is_safe_legacy_toolchain_line(trimmed: &str) -> bool {
    let guarded_nvm_source = matches!(
        trimmed,
        r#"[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh""#
            | r#"[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh""#
    );
    if trimmed.contains("PROTO_HOME") || trimmed.contains(".proto/") {
        return false;
    }
    [
        "export NVM_DIR=",
        "set -gx NVM_DIR ",
        "export FNM_DIR=",
        "export FNM_MULTISHELL_PATH=",
        "set -gx FNM_DIR ",
        "set -gx FNM_MULTISHELL_PATH ",
        "export PNPM_HOME=",
        "set -gx PNPM_HOME ",
    ]
    .iter()
    .any(|prefix| is_single_assignment(trimmed, prefix))
        || matches!(
            trimmed,
            r#". "$NVM_DIR/nvm.sh""# | r#"source "$NVM_DIR/nvm.sh""#
        )
        || guarded_nvm_source
}

fn is_single_assignment(line: &str, prefix: &str) -> bool {
    let Some(value) = line.strip_prefix(prefix).filter(|value| !value.is_empty()) else {
        return false;
    };
    if value.contains("$(") || value.contains('`') {
        return false;
    }
    if let Some(quote @ ('\'' | '"')) = value.chars().next() {
        return value.len() >= 2
            && value.ends_with(quote)
            && !value[1..value.len() - 1].contains(quote);
    }
    !value.chars().any(|character| {
        character.is_whitespace()
            || matches!(character, ';' | '&' | '|' | '<' | '>' | '#' | '(' | ')')
    })
}

fn fish_block_delta(line: &str) -> isize {
    line.split(';').fold(0, |depth, command| {
        let keyword = command.split_whitespace().next().unwrap_or_default();
        match keyword {
            "if" | "for" | "while" | "switch" | "function" | "begin" => depth + 1,
            "end" => depth - 1,
            _ => depth,
        }
    })
}

fn remove_fish_matching_lines(content: &str, should_remove: impl Fn(&str) -> bool) -> String {
    let source = content.lines().collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut index = 0;
    while index < source.len() {
        let line = source[index];
        if !should_remove(line.trim()) {
            result.push(line);
            index += 1;
            continue;
        }

        let mut depth = fish_block_delta(line);
        index += 1;
        while depth > 0 && index < source.len() {
            depth += fish_block_delta(source[index]);
            index += 1;
        }
    }
    result.join("\n")
}

fn preserved_variable_reference(content: &str, variable: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.contains(variable) && !is_safe_legacy_toolchain_line(trimmed)
    })
}

#[cfg(test)]
pub fn remove_legacy_toolchain_lines(content: &str, shell: ShellKind) -> String {
    let home = Path::new("/home/me");
    let nvm_roots = [home.join(".nvm")];
    let fnm_roots = [
        home.join(".fnm"),
        home.join(".local/share/fnm"),
        home.join("Library/Application Support/fnm"),
    ];
    let fnm_multishell_roots = [
        home.join(".local/state/fnm_multishells"),
        home.join("Library/Caches/fnm_multishells"),
    ];
    let pnpm_roots = [home.join("Library/pnpm"), home.join(".local/share/pnpm")];
    remove_legacy_toolchain_lines_scoped(
        content,
        shell,
        &LegacyCleanupScope {
            home,
            nvm_roots: &nvm_roots,
            fnm_roots: &fnm_roots,
            fnm_multishell_roots: &fnm_multishell_roots,
            pnpm_roots: &pnpm_roots,
            remove_manager_block: true,
        },
    )
}

pub fn remove_legacy_toolchain_lines_scoped(
    content: &str,
    shell: ShellKind,
    scope: &LegacyCleanupScope<'_>,
) -> String {
    let mut lines = Vec::new();
    let without_vite = remove_vite_loaders(content);
    let custom_nvm =
        has_unselected_assignment(&without_vite, "NVM_DIR", scope.home, scope.nvm_roots);
    let custom_fnm =
        has_unselected_assignment(&without_vite, "FNM_DIR", scope.home, scope.fnm_roots)
            || has_unselected_assignment(
                &without_vite,
                "FNM_MULTISHELL_PATH",
                scope.home,
                scope.fnm_multishell_roots,
            );
    let custom_pnpm =
        has_unselected_assignment(&without_vite, "PNPM_HOME", scope.home, scope.pnpm_roots);
    let remove_nvm = !scope.nvm_roots.is_empty() && !custom_nvm;
    let remove_fnm = !scope.fnm_roots.is_empty() && !custom_fnm;
    let remove_pnpm = !scope.pnpm_roots.is_empty() && !custom_pnpm;
    let without_block = if scope.remove_manager_block && remove_nvm && remove_fnm {
        remove_manager_block(&without_vite)
    } else {
        without_vite
    };
    let keep_nvm = preserved_variable_reference(&without_block, "NVM_DIR");
    let keep_fnm = preserved_variable_reference(&without_block, "FNM_DIR");
    let keep_fnm_multishell = preserved_variable_reference(&without_block, "FNM_MULTISHELL_PATH");
    let keep_pnpm = preserved_variable_reference(&without_block, "PNPM_HOME");
    let removable = |trimmed: &str| {
        let selected = remove_nvm
            && (assignment_matches(trimmed, "NVM_DIR", scope.home, scope.nvm_roots)
                || trimmed.contains("nvm.sh"))
            || remove_fnm
                && (assignment_matches(trimmed, "FNM_DIR", scope.home, scope.fnm_roots)
                    || assignment_matches(
                        trimmed,
                        "FNM_MULTISHELL_PATH",
                        scope.home,
                        scope.fnm_multishell_roots,
                    ))
            || remove_pnpm
                && assignment_matches(trimmed, "PNPM_HOME", scope.home, scope.pnpm_roots);
        selected
            && is_safe_legacy_toolchain_line(trimmed)
            && !(keep_nvm
                && (trimmed.starts_with("export NVM_DIR=")
                    || trimmed.starts_with("set -gx NVM_DIR ")))
            && !(keep_fnm
                && (trimmed.starts_with("export FNM_DIR=")
                    || trimmed.starts_with("set -gx FNM_DIR ")))
            && !(keep_fnm_multishell
                && (trimmed.starts_with("export FNM_MULTISHELL_PATH=")
                    || trimmed.starts_with("set -gx FNM_MULTISHELL_PATH ")))
            && !(keep_pnpm
                && (trimmed.starts_with("export PNPM_HOME=")
                    || trimmed.starts_with("set -gx PNPM_HOME ")))
    };
    if shell == ShellKind::Fish {
        return remove_fish_matching_lines(&without_block, removable).replace("\n\n\n", "\n\n");
    }
    for line in without_block.lines() {
        let trimmed = line.trim();
        if removable(trimmed) {
            continue;
        }
        lines.push(line);
    }
    let joined = lines.join("\n");
    joined.replace("\n\n\n", "\n\n")
}

fn has_unselected_assignment(
    content: &str,
    variable: &str,
    home: &Path,
    selected: &[PathBuf],
) -> bool {
    content.lines().any(|line| {
        assignment_value(line.trim(), variable).is_some_and(|value| {
            resolve_home_path(value, home).is_none_or(|path| !selected.contains(&path))
        })
    })
}

fn assignment_matches(line: &str, variable: &str, home: &Path, selected: &[PathBuf]) -> bool {
    assignment_value(line, variable)
        .and_then(|value| resolve_home_path(value, home))
        .is_some_and(|path| selected.contains(&path))
}

fn assignment_value<'a>(line: &'a str, variable: &str) -> Option<&'a str> {
    let export = format!("export {variable}=");
    let fish = format!("set -gx {variable} ");
    line.strip_prefix(&export)
        .or_else(|| line.strip_prefix(&fish))
        .filter(|value| !value.is_empty())
}

fn resolve_home_path(value: &str, home: &Path) -> Option<PathBuf> {
    let value = match (value.chars().next(), value.chars().last()) {
        (Some(open @ ('\'' | '"')), Some(close)) if open == close && value.len() >= 2 => {
            &value[1..value.len() - 1]
        }
        _ => value,
    };
    if value == "$HOME" || value == "${HOME}" {
        return Some(home.to_path_buf());
    }
    if let Some(relative) = value
        .strip_prefix("$HOME/")
        .or_else(|| value.strip_prefix("${HOME}/"))
    {
        return Some(home.join(relative));
    }
    Path::new(value).is_absolute().then(|| PathBuf::from(value))
}

pub fn shell_config_paths(home: &Path, zdotdir: Option<&str>) -> Vec<(ShellKind, PathBuf)> {
    let zsh_dir = zdotdir
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.starts_with(home))
        .unwrap_or_else(|| home.to_path_buf());
    vec![
        (ShellKind::Zsh, zsh_dir.join(".zshenv")),
        (ShellKind::Zsh, zsh_dir.join(".zshrc")),
        (ShellKind::Zsh, zsh_dir.join(".zprofile")),
        (ShellKind::Bash, home.join(".bashrc")),
        (ShellKind::Bash, home.join(".bash_profile")),
        (ShellKind::Bash, home.join(".profile")),
        (ShellKind::Fish, home.join(".config/fish/config.fish")),
        (ShellKind::Fish, home.join(".config/fish/conf.d/fnm.fish")),
        (
            ShellKind::Fish,
            home.join(".config/fish/conf.d/nlab-node-env-init.fish"),
        ),
        (
            ShellKind::Fish,
            home.join(".config/fish/conf.d/vite-plus.fish"),
        ),
    ]
}

pub fn validate_zdotdir(home: &Path, environment: &BTreeMap<OsString, OsString>) -> Result<()> {
    let Some(value) = value(environment, "ZDOTDIR").filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    let path = PathBuf::from(value);
    let relative = path.strip_prefix(home).map_err(|_| {
        AppError::UnsafePath(format!(
            "ZDOTDIR must be an absolute path inside HOME: {}",
            path.display()
        ))
    })?;
    if !path.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        return Err(AppError::UnsafePath(format!(
            "ZDOTDIR must be an absolute path inside HOME: {}",
            path.display()
        )));
    }
    let mut current = home.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::UnsafePath(format!(
                    "ZDOTDIR contains symlink component: {}",
                    current.display()
                )));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(AppError::UnsafePath(format!(
                    "ZDOTDIR component is not a directory: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(AppError::io("inspect ZDOTDIR", Some(current), error));
            }
        }
    }
    Ok(())
}

pub fn shell_config_plan(
    home: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Vec<(ShellKind, PathBuf, bool)> {
    let zdotdir = value(environment, "ZDOTDIR");
    let current_shell = value(environment, "SHELL");
    let configs = shell_config_paths(home, zdotdir.as_deref());
    let zsh = first_executable("zsh", environment).is_some()
        || current_shell
            .as_deref()
            .is_some_and(|shell| shell.ends_with("zsh"));
    let fish = first_executable("fish", environment).is_some()
        || current_shell
            .as_deref()
            .is_some_and(|shell| shell.ends_with("fish"));
    let bash = current_shell
        .as_deref()
        .is_some_and(|shell| shell.ends_with("bash"));
    let existing_bash = configs
        .iter()
        .any(|(shell, path)| *shell == ShellKind::Bash && path.exists());
    configs
        .into_iter()
        .map(|(shell, path)| {
            let file_name = path.file_name().and_then(|name| name.to_str());
            let enabled = match shell {
                ShellKind::Zsh => zsh && file_name == Some(".zshenv"),
                ShellKind::Fish => fish && file_name == Some("vite-plus.fish"),
                ShellKind::Bash => {
                    path.exists() || bash && !existing_bash && file_name == Some(".bashrc")
                }
            };
            (shell, path, enabled)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString, fs, path::Path};

    use super::{
        ShellKind, reconcile_vite_loader, remove_legacy_toolchain_lines,
        remove_legacy_toolchain_lines_scoped, remove_manager_block, remove_vite_loaders,
        shell_config_paths, shell_config_plan, upsert_vite_block_last, validate_zdotdir,
    };

    #[test]
    fn vite_block_moves_to_file_end_idempotently() {
        let original = "export X=1\n# >>> nlab-node-env-init vite-plus >>>\nold\n# <<< nlab-node-env-init vite-plus <<<\nexport Y=2\n";
        let next = upsert_vite_block_last(original, ShellKind::Zsh);

        assert!(next.starts_with("export X=1\nexport Y=2"));
        assert_eq!(next.matches("jt node init vite-plus >>>").count(), 1);
        assert_eq!(next, upsert_vite_block_last(&next, ShellKind::Zsh));
    }

    #[test]
    fn manager_removal_converges_legacy_and_current_blocks() {
        let content = "keep\n# >>> nlab-node-env-init fnm >>>\nold\n# <<< nlab-node-env-init <<<\n# >>> nlab-node-env-init node-auto-switch >>>\nnew\n# <<< nlab-node-env-init node-auto-switch <<<\nlast\n";

        assert_eq!(remove_manager_block(content), "keep\nlast");
        assert_eq!(
            remove_legacy_toolchain_lines(content, ShellKind::Zsh),
            "keep\nlast"
        );
    }

    #[test]
    fn vite_upsert_replaces_upstream_and_cli_loaders() {
        let original = "# Vite+ bin (https://viteplus.dev)\n. \"$HOME/.vite-plus/env\"\n# >>> nlab-node-env-init vite-plus >>>\nold\n# <<< nlab-node-env-init vite-plus <<<\n";
        let next = upsert_vite_block_last(original, ShellKind::Zsh);

        assert!(!next.contains("# Vite+ bin"));
        assert_eq!(next.matches("jt node init vite-plus >>>").count(), 1);
        assert_eq!(next.matches("$VP_HOME/env").count(), 2);
    }

    #[test]
    fn disabled_vite_target_removes_every_known_loader() {
        let original = "keep\n# Vite+ bin (https://viteplus.dev)\nsource \"$HOME/.vite-plus/env.fish\"\n# >>> nlab-node-env-init vite-plus >>>\nold\n# <<< nlab-node-env-init vite-plus <<<\n";

        assert_eq!(
            reconcile_vite_loader(original, ShellKind::Fish, false),
            "keep\n"
        );
    }

    #[test]
    fn malformed_vite_marker_does_not_delete_following_user_content() {
        let original =
            "keep\n# >>> jt node init vite-plus >>>\nunterminated\nexport USER_CONTENT=1\n";

        let next = super::remove_marker_block(original);

        assert_eq!(
            next,
            "keep\n# >>> jt node init vite-plus >>>\nunterminated\nexport USER_CONTENT=1"
        );
    }

    #[test]
    fn nested_malformed_marker_preserves_outer_user_content() {
        let original = "keep\n# >>> jt node init vite-plus >>>\nuser-content\n# >>> jt node init vite-plus >>>\nmanaged\n# <<< jt node init vite-plus <<<\nlast\n";

        let next = super::remove_marker_block(original);

        assert!(next.contains("user-content"));
        assert!(next.contains("last"));
    }

    #[test]
    fn fish_session_targets_zshenv_when_zsh_is_available() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let bin = home.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("zsh"), "").unwrap();
        fs::write(bin.join("fish"), "").unwrap();
        let environment = BTreeMap::from([
            (
                OsString::from("SHELL"),
                OsString::from("/opt/homebrew/bin/fish"),
            ),
            (OsString::from("PATH"), bin.as_os_str().to_os_string()),
        ]);
        let plan = shell_config_plan(home, &environment);

        assert!(plan.contains(&(ShellKind::Zsh, home.join(".zshenv"), true)));
        assert!(plan.contains(&(
            ShellKind::Fish,
            home.join(".config/fish/conf.d/vite-plus.fish"),
            true
        )));
        assert!(plan.contains(&(ShellKind::Zsh, home.join(".zshrc"), false)));
    }

    #[test]
    fn shell_paths_respect_safe_zdotdir() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let zdotdir = home.join("zsh");
        let paths = shell_config_paths(home, zdotdir.to_str());

        assert!(paths.contains(&(ShellKind::Zsh, zdotdir.join(".zshenv"))));
    }

    #[test]
    fn rejects_zdotdir_outside_home() {
        let root = tempfile::tempdir().unwrap();
        let environment =
            BTreeMap::from([(OsString::from("ZDOTDIR"), OsString::from("/outside/zsh"))]);

        assert!(validate_zdotdir(root.path(), &environment).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_zdotdir_before_mutation() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let zdotdir = home.path().join("zsh");
        symlink(outside.path(), &zdotdir).unwrap();
        let environment = BTreeMap::from([(
            OsString::from("ZDOTDIR"),
            zdotdir.as_os_str().to_os_string(),
        )]);

        assert!(validate_zdotdir(home.path(), &environment).is_err());
    }

    #[test]
    fn legacy_cleanup_preserves_proto_line() {
        let next = remove_legacy_toolchain_lines(
            "export NVM_DIR=\"$HOME/.nvm\"\nexport PROTO_HOME=$HOME/.proto\n",
            ShellKind::Zsh,
        );

        assert!(!next.contains("NVM_DIR"));
        assert!(next.contains("PROTO_HOME"));
    }

    #[test]
    fn fish_cleanup_removes_pnpm_header_block_without_orphan_end() {
        let source = "# pnpm\nset -gx PNPM_HOME \"$HOME/Library/pnpm\"\nif not string match -q -- \"$PNPM_HOME/bin\" $PATH\n  set -gx PATH \"$PNPM_HOME/bin\" $PATH\nend\n# pnpm end\nset -gx KEEP 1\n";
        let next = upsert_vite_block_last(
            &remove_legacy_toolchain_lines(source, ShellKind::Fish),
            ShellKind::Fish,
        );

        assert!(next.contains("set -gx PNPM_HOME"));
        assert!(next.contains("if not string match"));
        assert!(next.contains("\nend\n"));
        assert!(next.contains("set -gx KEEP 1"));
        assert!(next.contains("env.fish"));
    }

    #[test]
    fn fish_cleanup_preserves_unrelated_outer_block() {
        let source = "if status is-interactive\n  set -gx PNPM_HOME \"$HOME/Library/pnpm\"\n  echo keep\nend\n";
        let next = remove_legacy_toolchain_lines(source, ShellKind::Fish);

        assert_eq!(next, "if status is-interactive\n  echo keep\nend");
    }

    #[test]
    fn fish_cleanup_preserves_complex_pnpm_condition() {
        let source = "if test -d \"$PNPM_HOME\"\n  if test -x \"$PNPM_HOME/pnpm\"\n    echo old\n  end\nend\necho keep\n";
        let next = remove_legacy_toolchain_lines(source, ShellKind::Fish);

        assert_eq!(next, source.trim_end());
    }

    #[test]
    fn pnpm_only_cleanup_keeps_other_shell_content() {
        let next = remove_legacy_toolchain_lines(
            "export PNPM_HOME=$HOME/Library/pnpm\nexport KEEP=1\n",
            ShellKind::Zsh,
        );

        assert!(!next.contains("PNPM_HOME"));
        assert!(next.contains("KEEP=1"));
    }

    #[test]
    fn retained_pnpm_provider_keeps_shell_definition() {
        let source = "export PNPM_HOME=$HOME/Library/pnpm\n";
        let home = Path::new("/home/me");
        let nvm_roots = [home.join(".nvm")];
        let fnm_roots = [home.join(".local/share/fnm")];
        let fnm_multishell_roots = [home.join(".local/state/fnm_multishells")];

        assert_eq!(
            remove_legacy_toolchain_lines_scoped(
                source,
                ShellKind::Zsh,
                &super::LegacyCleanupScope {
                    home,
                    nvm_roots: &nvm_roots,
                    fnm_roots: &fnm_roots,
                    fnm_multishell_roots: &fnm_multishell_roots,
                    pnpm_roots: &[],
                    remove_manager_block: true,
                },
            ),
            source.trim_end()
        );
    }

    #[test]
    fn cleanup_preserves_unselected_shell_assignments() {
        let home = Path::new("/home/me");
        let nvm_roots = [home.join(".nvm")];
        let fnm_roots = [home.join(".local/share/fnm")];
        let fnm_multishell_roots = [home.join(".local/state/fnm_multishells")];
        let pnpm_roots = [home.join("Library/pnpm")];
        let source = "export FNM_DIR=\"$HOME/custom-fnm\"\nexport PNPM_HOME=\"$HOME/custom-pnpm\"\n# >>> nlab-node-env-init node-auto-switch >>>\neval \"$(fnm env --use-on-cd --shell zsh)\"\n# <<< nlab-node-env-init node-auto-switch <<<\n";

        assert_eq!(
            remove_legacy_toolchain_lines_scoped(
                source,
                ShellKind::Zsh,
                &super::LegacyCleanupScope {
                    home,
                    nvm_roots: &nvm_roots,
                    fnm_roots: &fnm_roots,
                    fnm_multishell_roots: &fnm_multishell_roots,
                    pnpm_roots: &pnpm_roots,
                    remove_manager_block: true,
                },
            ),
            source.trim_end()
        );
    }

    #[test]
    fn mixed_pnpm_path_keeps_unrelated_entries() {
        let source = "export PATH=\"$HOME/bin:$PNPM_HOME:$PATH\"\n";

        assert_eq!(
            remove_legacy_toolchain_lines(source, ShellKind::Zsh),
            source.trim_end()
        );
    }

    #[test]
    fn complex_posix_condition_remains_syntactically_whole() {
        let source = "if [ -d \"$PNPM_HOME\" ]; then\n  echo \"$PNPM_HOME\"\nfi\nexport KEEP=1\n";

        assert_eq!(
            remove_legacy_toolchain_lines(source, ShellKind::Zsh),
            source.trim_end()
        );
    }

    #[test]
    fn compound_vite_and_pnpm_lines_keep_user_commands() {
        let vite = r#"source "$VP_HOME/env"; export KEEP_VITE=1"#;

        assert_eq!(remove_vite_loaders(vite), vite);
        for pnpm in [
            r#"export PNPM_HOME="$HOME/Library/pnpm"; export KEEP_PNPM=1"#,
            r#"export PNPM_HOME="$HOME/Library/pnpm" KEEP_PNPM=1"#,
            r#"export PNPM_HOME=x&export KEEP_PNPM=1"#,
        ] {
            assert_eq!(remove_legacy_toolchain_lines(pnpm, ShellKind::Zsh), pnpm);
        }
    }
}
