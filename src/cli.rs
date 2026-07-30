use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::node::error::{AppError, Result};
use crate::node::fs::{atomic_write, read_optional};
use crate::node::platform::HomePaths;

const MANAGED_OPEN: &str = "# >>> jt cli bootstrap >>>";
const MANAGED_CLOSE: &str = "# <<< jt cli bootstrap <<<";
const GHOSTTY_CONFIG: &str = include_str!("../assets/cli/ghostty.conf");
const FISH_CONFIG: &str = include_str!("../assets/cli/bootstrap.fish");
const FISH_GIT_SHORTCUTS: &str = include_str!("../assets/cli/git-shortcuts.fish");
const FISH_PROXY_ON: &str = include_str!("../assets/cli/proxy-on.fish");
const FISH_PROXY_OFF: &str = include_str!("../assets/cli/proxy-off.fish");
const FISH_PROXY_AUTO_ON: &str = include_str!("../assets/cli/proxy-auto-on.fish");
const FISH_PROXY_AUTO_OFF: &str = include_str!("../assets/cli/proxy-auto-off.fish");
const ZSH_CONFIG: &str = include_str!("../assets/cli/bootstrap.zsh");
const STARSHIP_CONFIG: &str = include_str!("../assets/cli/starship.toml");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Platform {
    Macos,
    Debian { wsl: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shell {
    Fish,
    Zsh,
}

impl Shell {
    fn name(self) -> &'static str {
        match self {
            Self::Fish => "Fish",
            Self::Zsh => "Zsh",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Fish => "fish",
            Self::Zsh => "zsh",
        }
    }
}

pub fn bootstrap() -> u8 {
    if !std::io::stdin().is_terminal() {
        eprintln!("error: jt cli bootstrap 仅支持交互运行");
        return 1;
    }

    match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn run() -> Result<()> {
    let environment = env::vars_os().collect::<BTreeMap<_, _>>();
    let home = HomePaths::from_environment(&environment)?.home;
    let platform = detect_platform()?;

    cliclack::intro("jt cli bootstrap")
        .map_err(|error| AppError::io("render intro", None, error))?;
    cliclack::note(
        "终端工具链",
        "Ghostty + Fish/Zsh + Starship + Maple Mono NF CN\n\
         bat/eza/fd/ripgrep/fzf/btop/zoxide/jq/tldr/delta/lazygit\n\
         不安装 Node、fnm、pnpm",
    )
    .map_err(|error| AppError::io("render bootstrap summary", None, error))?;

    let shell = cliclack::select("选择默认 Shell")
        .initial_value(Shell::Fish)
        .item(Shell::Fish, "Fish", "内置补全、建议、语法高亮")
        .item(Shell::Zsh, "Zsh", "POSIX 风格，加载轻量插件")
        .interact()
        .map_err(|error| AppError::io("select shell", None, error))?;
    let install_zellij = cliclack::confirm("安装 Zellij？")
        .initial_value(false)
        .interact()
        .map_err(|error| AppError::io("select Zellij", None, error))?;
    let auto_proxy = if shell == Shell::Fish {
        cliclack::confirm("启动 Fish 时自动执行 proxy-on？")
            .initial_value(false)
            .interact()
            .map_err(|error| AppError::io("select Fish proxy autostart", None, error))?
    } else {
        false
    };
    let fish_changes = if shell == Shell::Fish {
        format!(
            "\n部署 Git 快捷指令和 proxy-on/proxy-off\nFish 自动代理：{}",
            if auto_proxy { "开启" } else { "关闭" }
        )
    } else {
        String::new()
    };

    cliclack::note(
        "将执行",
        format!(
            "安装系统软件包\n设置 {} 为默认 Shell\n写入 jt 托管配置并备份被修改文件{}\n更新 Git delta 全局配置",
            shell.name(),
            fish_changes
        ),
    )
    .map_err(|error| AppError::io("render mutation summary", None, error))?;
    if !cliclack::confirm("继续？")
        .initial_value(false)
        .interact()
        .map_err(|error| AppError::io("read bootstrap confirmation", None, error))?
    {
        cliclack::outro_cancel("未改动系统")
            .map_err(|error| AppError::io("render cancellation", None, error))?;
        return Ok(());
    }

    match platform {
        Platform::Macos => install_macos(shell, install_zellij)?,
        Platform::Debian { .. } => install_debian(&home, shell, install_zellij)?,
    }
    set_default_shell(shell)?;
    deploy_configs(&home, platform, shell, auto_proxy)?;
    configure_delta()?;

    let terminal = match platform {
        Platform::Macos => "打开 Ghostty",
        Platform::Debian { wsl: true } => "在 Windows 侧安装 Ghostty 或 Windows Terminal",
        Platform::Debian { wsl: false } => "按发行版文档安装 Ghostty",
    };
    cliclack::outro(format!(
        "完成。{terminal}；新 Shell 生效后运行 `exec {}`",
        shell.command()
    ))
    .map_err(|error| AppError::io("render completion", None, error))
}

fn detect_platform() -> Result<Platform> {
    if !matches!(env::consts::ARCH, "x86_64" | "aarch64") {
        return Err(AppError::Invalid(format!(
            "unsupported architecture before mutation: {}",
            env::consts::ARCH
        )));
    }
    match env::consts::OS {
        "macos" => Ok(Platform::Macos),
        "linux" => {
            let release = fs::read_to_string("/etc/os-release")
                .map_err(|error| AppError::io("read Linux distribution", None, error))?;
            if !is_debian_family(&release) {
                return Err(AppError::Invalid(
                    "jt cli bootstrap 仅支持 Debian/Ubuntu Linux".to_owned(),
                ));
            }
            let wsl = fs::read_to_string("/proc/version")
                .map(|version| version.to_ascii_lowercase().contains("microsoft"))
                .unwrap_or(false);
            Ok(Platform::Debian { wsl })
        }
        os => Err(AppError::Invalid(format!(
            "unsupported platform before mutation: {os}/{}",
            env::consts::ARCH
        ))),
    }
}

fn is_debian_family(os_release: &str) -> bool {
    os_release.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        line == "id=debian"
            || line == "id=ubuntu"
            || line == "id=\"debian\""
            || line == "id=\"ubuntu\""
            || (line.starts_with("id_like=")
                && (line.contains("debian") || line.contains("ubuntu")))
    })
}

fn install_macos(shell: Shell, install_zellij: bool) -> Result<()> {
    let brew = ensure_homebrew()?;
    if !Path::new("/Applications/Ghostty.app").exists()
        && !command_success(&brew, ["list", "--cask", "ghostty"])
    {
        run_command(&brew, ["install", "--cask", "ghostty"], "install Ghostty")?;
    }
    if !command_success(&brew, ["list", "--cask", "font-maple-mono-nf-cn"]) {
        run_command(
            &brew,
            ["install", "--cask", "font-maple-mono-nf-cn"],
            "install Maple Mono NF CN",
        )?;
    }

    let mut packages = vec![
        "bat",
        "eza",
        "fd",
        "ripgrep",
        "btop",
        "zoxide",
        "jq",
        "tldr",
        "git-delta",
        "lazygit",
        "fzf",
        "starship",
    ];
    match shell {
        Shell::Fish => packages.push("fish"),
        Shell::Zsh => packages.extend([
            "zsh-autosuggestions",
            "zsh-syntax-highlighting",
            "zsh-completions",
        ]),
    }
    if install_zellij {
        packages.push("zellij");
    }
    let mut arguments = vec!["install"];
    arguments.extend(packages);
    run_command(&brew, arguments, "install terminal CLI tools")
}

fn ensure_homebrew() -> Result<PathBuf> {
    if let Some(brew) = find_command("brew") {
        return Ok(brew);
    }
    run_command(
        "/bin/bash",
        [
            "-c",
            "set -o pipefail; curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh | /bin/bash",
        ],
        "install Homebrew",
    )?;
    find_command("brew")
        .ok_or_else(|| AppError::Invalid("Homebrew installed but brew was not found".to_owned()))
}

fn install_debian(home: &Path, shell: Shell, install_zellij: bool) -> Result<()> {
    run_privileged("apt-get", &["update"], "update apt package index")?;
    let mut packages = vec![
        "ca-certificates",
        "curl",
        "git",
        "unzip",
        "fontconfig",
        "bat",
        "fd-find",
        "ripgrep",
        "jq",
        "fzf",
    ];
    match shell {
        Shell::Fish => packages.push("fish"),
        Shell::Zsh => packages.extend(["zsh", "zsh-autosuggestions", "zsh-syntax-highlighting"]),
    }
    let mut arguments = vec!["install", "-y"];
    arguments.extend(packages);
    run_privileged("apt-get", &arguments, "install required terminal packages")?;

    for package in ["btop", "zoxide", "eza", "tealdeer", "git-delta", "lazygit"] {
        if !try_privileged("apt-get", &["install", "-y", package]) {
            eprintln!("warning: apt 没有 {package}；跳过，请按发行版安装");
        }
    }
    install_maple_font(home)?;
    install_release_binary(
        home,
        "starship",
        &format!(
            "starship-{}-unknown-linux-musl.tar.gz",
            linux_architecture()
        ),
        ".sha256",
        "https://github.com/starship/starship/releases/latest/download",
    )?;
    if install_zellij {
        install_release_binary(
            home,
            "zellij",
            &format!("zellij-{}-unknown-linux-musl.tar.gz", linux_architecture()),
            ".sha256sum",
            "https://github.com/zellij-org/zellij/releases/latest/download",
        )?;
    }
    Ok(())
}

fn linux_architecture() -> &'static str {
    match env::consts::ARCH {
        "aarch64" => "aarch64",
        _ => "x86_64",
    }
}

fn install_maple_font(home: &Path) -> Result<()> {
    if command_output(
        "fc-list",
        std::iter::empty::<&str>(),
        "inspect installed fonts",
    )?
    .stdout
    .windows("Maple Mono NF CN".len())
    .any(|window| window == b"Maple Mono NF CN")
    {
        return Ok(());
    }

    let temp = tempfile::tempdir()
        .map_err(|error| AppError::io("create font temporary directory", None, error))?;
    let archive = temp.path().join("MapleMono-NF-CN-unhinted.zip");
    let checksum = temp.path().join("MapleMono-NF-CN-unhinted.sha256");
    download(
        "https://github.com/subframe7536/maple-font/releases/latest/download/MapleMono-NF-CN-unhinted.zip",
        &archive,
    )?;
    download(
        "https://github.com/subframe7536/maple-font/releases/latest/download/MapleMono-NF-CN-unhinted.sha256",
        &checksum,
    )?;
    verify_sha256(&archive, &checksum)?;

    let extracted = temp.path().join("fonts");
    fs::create_dir(&extracted)
        .map_err(|error| AppError::io("create font extraction directory", None, error))?;
    run_command(
        "unzip",
        [
            OsString::from("-q"),
            OsString::from("-j"),
            archive.as_os_str().to_os_string(),
            OsString::from("*.ttf"),
            OsString::from("-d"),
            extracted.as_os_str().to_os_string(),
        ],
        "extract Maple Mono NF CN",
    )?;

    let font_directory = home.join(".local/share/fonts/MapleMono-NF-CN");
    let mut installed = 0;
    for entry in fs::read_dir(&extracted)
        .map_err(|error| AppError::io("read extracted fonts", Some(extracted.clone()), error))?
    {
        let entry = entry.map_err(|error| AppError::io("read extracted font", None, error))?;
        if entry.path().extension() != Some(OsStr::new("ttf")) {
            continue;
        }
        let content = fs::read(entry.path())
            .map_err(|error| AppError::io("read extracted font", Some(entry.path()), error))?;
        write_managed_file(home, &font_directory.join(entry.file_name()), &content)?;
        installed += 1;
    }
    if installed == 0 {
        return Err(AppError::Invalid(
            "Maple Mono archive contained no TTF files".to_owned(),
        ));
    }
    run_command(
        "fc-cache",
        [
            OsString::from("-f"),
            font_directory.as_os_str().to_os_string(),
        ],
        "refresh font cache",
    )
}

fn install_release_binary(
    home: &Path,
    command: &str,
    archive_name: &str,
    checksum_suffix: &str,
    base_url: &str,
) -> Result<()> {
    if find_command(command).is_some() || home.join(".local/bin").join(command).is_file() {
        return Ok(());
    }
    let temp = tempfile::tempdir()
        .map_err(|error| AppError::io("create binary temporary directory", None, error))?;
    let archive = temp.path().join(archive_name);
    let checksum_name = if checksum_suffix == ".sha256sum" {
        archive_name.trim_end_matches(".tar.gz").to_owned() + checksum_suffix
    } else {
        archive_name.to_owned() + checksum_suffix
    };
    let checksum = temp.path().join(&checksum_name);
    download(&format!("{base_url}/{archive_name}"), &archive)?;
    download(&format!("{base_url}/{checksum_name}"), &checksum)?;
    verify_sha256(&archive, &checksum)?;
    run_command(
        "tar",
        [
            OsString::from("-xzf"),
            archive.as_os_str().to_os_string(),
            OsString::from("-C"),
            temp.path().as_os_str().to_os_string(),
        ],
        &format!("extract {command}"),
    )?;
    let source = temp.path().join(command);
    let content = fs::read(&source)
        .map_err(|error| AppError::io("read extracted binary", Some(source), error))?;
    let target = home.join(".local/bin").join(command);
    write_managed_file(home, &target, &content)?;
    make_executable(&target)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|error| AppError::io("read binary permissions", Some(path.to_path_buf()), error))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .map_err(|error| AppError::io("set binary executable", Some(path.to_path_buf()), error))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn download(url: &str, target: &Path) -> Result<()> {
    run_command(
        "curl",
        [
            OsString::from("-fsSL"),
            OsString::from("--retry"),
            OsString::from("3"),
            OsString::from("-o"),
            target.as_os_str().to_os_string(),
            OsString::from(url),
        ],
        "download release asset",
    )
}

fn verify_sha256(archive: &Path, checksum: &Path) -> Result<()> {
    let expected = fs::read_to_string(checksum)
        .map_err(|error| {
            AppError::io("read SHA-256 checksum", Some(checksum.to_path_buf()), error)
        })?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::Invalid(format!(
            "invalid SHA-256 checksum: {}",
            checksum.display()
        )));
    }
    let output = command_output(
        "sha256sum",
        [archive.as_os_str().to_os_string()],
        "hash release asset",
    )?;
    let actual = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if actual != expected {
        return Err(AppError::Invalid(format!(
            "SHA-256 mismatch for {}",
            archive.display()
        )));
    }
    Ok(())
}

fn set_default_shell(shell: Shell) -> Result<()> {
    let shell_path = find_command(shell.command())
        .ok_or_else(|| AppError::Invalid(format!("{} installed but not found", shell.name())))?;
    if env::var_os("SHELL").as_deref() == Some(shell_path.as_os_str()) {
        return Ok(());
    }
    let registered = fs::read_to_string("/etc/shells")
        .map(|content| content.lines().any(|line| Path::new(line) == shell_path))
        .unwrap_or(false);
    if !registered {
        append_system_shell(&shell_path)?;
    }
    run_command(
        "chsh",
        [OsString::from("-s"), shell_path.into_os_string()],
        "change default shell",
    )
}

fn append_system_shell(shell: &Path) -> Result<()> {
    let mut command = if is_root() {
        Command::new("tee")
    } else {
        let mut command = Command::new("sudo");
        command.arg("tee");
        command
    };
    let mut child = command
        .args(["-a", "/etc/shells"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| AppError::io("start /etc/shells update", None, error))?;
    child
        .stdin
        .take()
        .ok_or_else(|| AppError::Invalid("cannot write /etc/shells".to_owned()))?
        .write_all(format!("{}\n", shell.display()).as_bytes())
        .map_err(|error| AppError::io("write /etc/shells", None, error))?;
    let status = child
        .wait()
        .map_err(|error| AppError::io("wait for /etc/shells update", None, error))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::Command {
            action: "register default shell".to_owned(),
            status: status.code().unwrap_or(1),
            detail: "tee returned non-zero".to_owned(),
        })
    }
}

fn deploy_configs(home: &Path, platform: Platform, shell: Shell, auto_proxy: bool) -> Result<()> {
    let managed = home.join(".config/jt-cli");
    write_managed_file(
        home,
        &managed.join("starship.toml"),
        STARSHIP_CONFIG.as_bytes(),
    )?;
    match shell {
        Shell::Fish => {
            write_managed_file(
                home,
                &home.join(".config/fish/conf.d/jt-cli-bootstrap.fish"),
                FISH_CONFIG.as_bytes(),
            )?;
            write_managed_file(
                home,
                &home.join(".config/fish/conf.d/jt-cli-git-shortcuts.fish"),
                FISH_GIT_SHORTCUTS.as_bytes(),
            )?;
            write_managed_file(
                home,
                &home.join(".config/fish/functions/proxy-on.fish"),
                FISH_PROXY_ON.as_bytes(),
            )?;
            write_managed_file(
                home,
                &home.join(".config/fish/functions/proxy-off.fish"),
                FISH_PROXY_OFF.as_bytes(),
            )?;
            write_managed_file(
                home,
                &home.join(".config/fish/conf.d/jt-cli-proxy-auto.fish"),
                if auto_proxy {
                    FISH_PROXY_AUTO_ON
                } else {
                    FISH_PROXY_AUTO_OFF
                }
                .as_bytes(),
            )?;
        }
        Shell::Zsh => {
            write_managed_file(
                home,
                &managed.join("cli-bootstrap.zsh"),
                ZSH_CONFIG.as_bytes(),
            )?;
            write_managed_block(
                home,
                &home.join(".zshrc"),
                "source \"$HOME/.config/jt-cli/cli-bootstrap.zsh\"",
            )?;
        }
    }

    let ghostty_directory = match platform {
        Platform::Macos => home.join("Library/Application Support/com.mitchellh.ghostty"),
        Platform::Debian { .. } => home.join(".config/ghostty"),
    };
    write_managed_file(
        home,
        &ghostty_directory.join("jt-cli-bootstrap.ghostty"),
        GHOSTTY_CONFIG.as_bytes(),
    )?;
    let current = ghostty_directory.join("config.ghostty");
    let legacy = ghostty_directory.join("config");
    let entry = if current.exists() || !legacy.exists() {
        current
    } else {
        legacy
    };
    write_managed_block(home, &entry, "config-file = jt-cli-bootstrap.ghostty")
}

fn write_managed_block(home: &Path, path: &Path, body: &str) -> Result<()> {
    let current = read_optional(path)?;
    let current_text = current
        .as_deref()
        .map(|content| {
            std::str::from_utf8(content)
                .map(str::to_owned)
                .map_err(|error| AppError::Decode {
                    action: format!("decode {}", path.display()),
                    detail: error.to_string(),
                })
        })
        .transpose()?
        .unwrap_or_default();
    let next = upsert_managed_block(&current_text, body)?;
    if current_text == next {
        return Ok(());
    }
    backup(home, path, current.as_deref())?;
    atomic_write(home, path, current.as_deref(), next.as_bytes())
}

fn upsert_managed_block(source: &str, body: &str) -> Result<String> {
    let open_count = source.matches(MANAGED_OPEN).count();
    let close_count = source.matches(MANAGED_CLOSE).count();
    if open_count > 1 || close_count > 1 || open_count != close_count {
        return Err(AppError::Invalid(
            "jt cli bootstrap managed block is malformed; refusing to edit".to_owned(),
        ));
    }
    let block = format!("{MANAGED_OPEN}\n{}\n{MANAGED_CLOSE}", body.trim_end());
    if open_count == 0 {
        let separator = if source.is_empty() || source.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let leading = if source.is_empty() { "" } else { "\n" };
        return Ok(format!("{source}{separator}{leading}{block}\n"));
    }
    let open = source.find(MANAGED_OPEN).expect("count checked");
    let close = source.find(MANAGED_CLOSE).expect("count checked");
    if close < open {
        return Err(AppError::Invalid(
            "jt cli bootstrap managed block is malformed; refusing to edit".to_owned(),
        ));
    }
    let close_end = close + MANAGED_CLOSE.len();
    Ok(format!(
        "{}{}{}",
        &source[..open],
        block,
        &source[close_end..]
    ))
}

fn write_managed_file(home: &Path, path: &Path, content: &[u8]) -> Result<()> {
    let current = read_optional(path)?;
    if current.as_deref() == Some(content) {
        return Ok(());
    }
    backup(home, path, current.as_deref())?;
    atomic_write(home, path, current.as_deref(), content)
}

fn backup(home: &Path, path: &Path, current: Option<&[u8]>) -> Result<()> {
    let Some(current) = current else {
        return Ok(());
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::Invalid(format!("system clock before epoch: {error}")))?
        .as_millis();
    let name = path
        .file_name()
        .ok_or_else(|| AppError::Invalid(format!("file has no name: {}", path.display())))?
        .to_string_lossy();
    let backup = path.with_file_name(format!("{name}.bak.{timestamp}"));
    atomic_write(home, &backup, None, current)
}

fn configure_delta() -> Result<()> {
    if find_command("delta").is_none()
        && !env::var_os("HOME")
            .map(PathBuf::from)
            .is_some_and(|home| home.join(".local/bin/delta").is_file())
    {
        eprintln!("warning: delta 未安装；跳过 Git pager 配置");
        return Ok(());
    }
    for (key, value) in [
        ("core.pager", "delta"),
        ("interactive.diffFilter", "delta --color-only"),
        ("delta.navigate", "true"),
        ("delta.dark", "true"),
        ("delta.line-numbers", "true"),
        ("delta.side-by-side", "true"),
        ("merge.conflictstyle", "diff3"),
        ("diff.colorMoved", "default"),
    ] {
        run_command(
            "git",
            ["config", "--global", key, value],
            "configure git-delta",
        )?;
    }
    Ok(())
}

fn run_privileged(program: &str, args: &[&str], action: &str) -> Result<()> {
    if is_root() {
        run_command(program, args.iter().copied(), action)
    } else {
        run_command(
            "sudo",
            std::iter::once(program).chain(args.iter().copied()),
            action,
        )
    }
}

fn try_privileged(program: &str, args: &[&str]) -> bool {
    let status = if is_root() {
        Command::new(program).args(args).status()
    } else {
        Command::new("sudo").arg(program).args(args).status()
    };
    status.is_ok_and(|status| status.success())
}

fn is_root() -> bool {
    command_output("id", ["-u"], "read current user")
        .is_ok_and(|output| output.status.success() && output.stdout == b"0\n")
}

fn run_command(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    action: &str,
) -> Result<()> {
    let status = Command::new(program.as_ref())
        .args(args)
        .status()
        .map_err(|error| AppError::io(action, None, error))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::Command {
            action: action.to_owned(),
            status: status.code().unwrap_or(1),
            detail: "command returned non-zero".to_owned(),
        })
    }
}

fn command_output(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    action: &str,
) -> Result<Output> {
    Command::new(program.as_ref())
        .args(args)
        .output()
        .map_err(|error| AppError::io(action, None, error))
}

fn command_success(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> bool {
    Command::new(program.as_ref())
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn find_command(name: &str) -> Option<PathBuf> {
    let mut directories = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ]);
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        FISH_CONFIG, FISH_GIT_SHORTCUTS, FISH_PROXY_AUTO_OFF, FISH_PROXY_AUTO_ON, FISH_PROXY_OFF,
        FISH_PROXY_ON, GHOSTTY_CONFIG, MANAGED_CLOSE, MANAGED_OPEN, Platform, STARSHIP_CONFIG,
        Shell, ZSH_CONFIG, deploy_configs, is_debian_family, upsert_managed_block,
        write_managed_file,
    };

    #[test]
    fn bootstrap_assets_use_maple_and_have_no_node_setup() {
        let assets = [
            GHOSTTY_CONFIG,
            FISH_CONFIG,
            FISH_GIT_SHORTCUTS,
            FISH_PROXY_ON,
            FISH_PROXY_OFF,
            FISH_PROXY_AUTO_ON,
            FISH_PROXY_AUTO_OFF,
            ZSH_CONFIG,
            STARSHIP_CONFIG,
        ]
        .join("\n");

        assert!(assets.contains("Maple Mono NF CN"));
        for removed in ["MesloLGS", "fnm", "PNPM_HOME", "$nodejs", "[nodejs]"] {
            assert!(
                !assets.contains(removed),
                "found removed Node/font content: {removed}"
            );
        }
    }

    #[test]
    fn managed_block_is_idempotent_and_preserves_user_content() {
        let first = upsert_managed_block("before\n", "source managed").unwrap();
        let second = upsert_managed_block(&first, "source managed").unwrap();

        assert_eq!(first, second);
        assert!(first.starts_with("before\n"));
        assert!(first.contains(MANAGED_OPEN));
        assert!(first.contains(MANAGED_CLOSE));
    }

    #[test]
    fn malformed_managed_block_is_rejected() {
        assert!(upsert_managed_block(MANAGED_OPEN, "managed").is_err());
    }

    #[test]
    fn managed_file_keeps_backup_before_replacement() {
        let home = tempdir().unwrap();
        let target = home.path().join(".config/tool/config");
        write_managed_file(home.path(), &target, b"first\n").unwrap();
        write_managed_file(home.path(), &target, b"second\n").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"second\n");
        let backups = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak."))
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path()).unwrap(), b"first\n");
    }

    #[test]
    fn deploys_managed_zsh_and_ghostty_config_idempotently() {
        let home = tempdir().unwrap();
        fs::write(home.path().join(".zshrc"), "keep\n").unwrap();

        deploy_configs(home.path(), Platform::Macos, Shell::Zsh, false).unwrap();
        deploy_configs(home.path(), Platform::Macos, Shell::Zsh, false).unwrap();

        let zshrc = fs::read_to_string(home.path().join(".zshrc")).unwrap();
        assert!(zshrc.starts_with("keep\n"));
        assert_eq!(zshrc.matches(MANAGED_OPEN).count(), 1);
        assert!(home.path().join(".config/jt-cli/starship.toml").is_file());
        assert!(
            home.path()
                .join("Library/Application Support/com.mitchellh.ghostty/jt-cli-bootstrap.ghostty")
                .is_file()
        );
    }

    #[test]
    fn deploys_fish_shortcuts_and_selected_proxy_autostart() {
        let home = tempdir().unwrap();

        deploy_configs(home.path(), Platform::Macos, Shell::Fish, true).unwrap();
        deploy_configs(home.path(), Platform::Macos, Shell::Fish, true).unwrap();

        let fish = home.path().join(".config/fish");
        assert_eq!(
            fs::read_to_string(fish.join("conf.d/jt-cli-git-shortcuts.fish")).unwrap(),
            FISH_GIT_SHORTCUTS
        );
        assert_eq!(
            fs::read_to_string(fish.join("functions/proxy-on.fish")).unwrap(),
            FISH_PROXY_ON
        );
        assert_eq!(
            fs::read_to_string(fish.join("functions/proxy-off.fish")).unwrap(),
            FISH_PROXY_OFF
        );
        assert_eq!(
            fs::read_to_string(fish.join("conf.d/jt-cli-proxy-auto.fish")).unwrap(),
            FISH_PROXY_AUTO_ON
        );

        deploy_configs(home.path(), Platform::Macos, Shell::Fish, false).unwrap();
        assert_eq!(
            fs::read_to_string(fish.join("conf.d/jt-cli-proxy-auto.fish")).unwrap(),
            FISH_PROXY_AUTO_OFF
        );
    }

    #[test]
    fn recognizes_debian_family_only() {
        assert!(is_debian_family("ID=ubuntu\n"));
        assert!(is_debian_family(
            "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"
        ));
        assert!(!is_debian_family("ID=fedora\n"));
    }
}
