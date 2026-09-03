use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

fn nlab_api() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nlab-api"));
    command.env("NLAB_API_NO_UPDATE", "1");
    command
}

fn git(root: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {}", arguments.join(" "));
}

fn write(root: &Path, relative: &str, source: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

#[test]
fn exposes_public_commands() {
    let output = nlab_api().arg("--help").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success());
    assert!(stdout.contains("init"));
    assert!(stdout.contains("generate"));
    assert!(stdout.contains("config"));
    assert!(stdout.contains("update"));
    for hidden in ["routes", "migrate", "mock", "accept"] {
        assert!(!stdout.contains(hidden));
    }
}

#[test]
fn invalid_generate_project_is_non_mutating() {
    let project = tempdir().unwrap();

    let output = nlab_api()
        .args(["generate", "--project", project.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("read nlab-api config"));
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 0);
}

#[test]
fn standalone_config_persists_and_shows_explicit_runner() {
    let project = tempdir().unwrap();
    let project_path = project.path().to_str().unwrap();

    let configured = nlab_api()
        .args(["config", "--runner", "nlab-api", "--project", project_path])
        .output()
        .unwrap();
    assert!(configured.status.success());
    let local: serde_json::Value = serde_json::from_slice(
        &fs::read(project.path().join(".nlab/nlab-api.local.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(local["runner"], "nlab-api");

    let shown = nlab_api()
        .args(["config", "--show", "--project", project_path])
        .output()
        .unwrap();
    assert!(shown.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&shown.stdout).unwrap()["runner"],
        "nlab-api"
    );
}

#[cfg(unix)]
#[test]
fn config_detect_prefers_jt_then_falls_back_to_nlab_api() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().unwrap();
    let both = root.path().join("both");
    fs::create_dir(&both).unwrap();
    for command in ["jt", "nlab-api"] {
        let path = both.join(command);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let preferred = root.path().join("preferred");
    fs::create_dir(&preferred).unwrap();
    let detected = nlab_api()
        .args([
            "config",
            "--detect",
            "--project",
            preferred.to_str().unwrap(),
        ])
        .env("PATH", &both)
        .output()
        .unwrap();
    assert!(detected.status.success());
    let local: serde_json::Value =
        serde_json::from_slice(&fs::read(preferred.join(".nlab/nlab-api.local.json")).unwrap())
            .unwrap();
    assert_eq!(local["runner"], "jt");

    let standalone_bin = root.path().join("standalone-bin");
    fs::create_dir(&standalone_bin).unwrap();
    let standalone = standalone_bin.join("nlab-api");
    fs::write(&standalone, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(standalone, fs::Permissions::from_mode(0o755)).unwrap();
    let fallback = root.path().join("fallback");
    fs::create_dir(&fallback).unwrap();
    let detected = nlab_api()
        .args([
            "config",
            "--detect",
            "--project",
            fallback.to_str().unwrap(),
        ])
        .env("PATH", &standalone_bin)
        .output()
        .unwrap();
    assert!(detected.status.success());
    let local: serde_json::Value =
        serde_json::from_slice(&fs::read(fallback.join(".nlab/nlab-api.local.json")).unwrap())
            .unwrap();
    assert_eq!(local["runner"], "nlab-api");
}

#[cfg(unix)]
#[test]
fn init_clones_backend_splits_config_and_generate_switches_branches() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().unwrap();
    let remote = root.path().join("backend.git");
    let upstream = root.path().join("upstream");
    let backend = root.path().join("backend");
    let frontend = root.path().join("frontend");
    fs::create_dir(&upstream).unwrap();
    fs::create_dir(&frontend).unwrap();

    git(
        root.path(),
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );
    git(&upstream, &["init", "--initial-branch=main"]);
    git(&upstream, &["config", "user.name", "test"]);
    git(&upstream, &["config", "user.email", "test@example.com"]);
    write(
        &upstream,
        "contract/src/main/java/p/contract/checkapp/ITestFacade.java",
        "@ServiceContract public interface ITestFacade {}\n",
    );
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-m", "initial"]);
    git(
        &upstream,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&upstream, &["push", "-u", "origin", "main"]);
    git(&upstream, &["switch", "-c", "feature"]);
    write(
        &upstream,
        "contract/src/main/java/p/contract/checkapp/ITestFacade.java",
        "@ServiceContract public interface ITestFacade { String feature(); }\n",
    );
    git(&upstream, &["commit", "-am", "feature"]);
    git(&upstream, &["push", "-u", "origin", "feature"]);
    git(&upstream, &["switch", "main"]);

    write(&frontend, "package.json", r#"{"private":true}"#);
    write(
        &frontend,
        "vite.config.ts",
        "export default {\n  resolve: {\n    alias: {\n      '@': fileURLToPath(new URL('./src', import.meta.url)),\n    },\n  },\n}\n",
    );
    write(
        &frontend,
        "tsconfig.json",
        r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
    );
    write(
        &frontend,
        "src/utils/request.ts",
        "interface Response<T> {\n  code: string\n  data: T\n}\nexport function nlabRequest<T>(): Promise<T> { throw new Error() }\n",
    );

    let output = nlab_api()
        .args([
            "init",
            "--project",
            frontend.to_str().unwrap(),
            "--repo-url",
            remote.to_str().unwrap(),
            "--clone-dir",
            backend.to_str().unwrap(),
            "--branch",
            "main",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let shared: serde_json::Value =
        serde_json::from_slice(&fs::read(frontend.join(".nlab/nlab-api.config.json")).unwrap())
            .unwrap();
    assert_eq!(shared["version"], 2);
    assert_eq!(shared["backend"]["repository"], remote.to_str().unwrap());
    assert!(shared["backend"].get("repoPath").is_none());
    assert_eq!(shared["backend"]["branch"], "main");
    assert_eq!(shared["backend"]["appName"], "backend");

    let local: serde_json::Value =
        serde_json::from_slice(&fs::read(frontend.join(".nlab/nlab-api.local.json")).unwrap())
            .unwrap();
    assert_eq!(local["version"], 1);
    assert_eq!(
        local["backend"]["repoPath"],
        backend.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(local.get("runner").is_none());
    assert_eq!(
        fs::read_to_string(frontend.join(".gitignore")).unwrap(),
        "/.nlab/nlab-api.local.json\n"
    );

    let fake_bin = root.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    let codegraph = fake_bin.join("codegraph");
    fs::write(&codegraph, "#!/bin/sh\nexit 9\n").unwrap();
    fs::set_permissions(&codegraph, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var_os("PATH")
            .unwrap_or_default()
            .to_string_lossy()
    );

    let feature = nlab_api()
        .args([
            "generate",
            "--project",
            frontend.to_str().unwrap(),
            "--branch",
            "feature",
            "--timeout-seconds",
            "10",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(feature.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&feature.stderr).contains("codegraph init failed"));
    assert_eq!(
        String::from_utf8(
            Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(&backend)
                .output()
                .unwrap()
                .stdout
        )
        .unwrap()
        .trim(),
        "feature"
    );

    let main = nlab_api()
        .args([
            "generate",
            "--project",
            frontend.to_str().unwrap(),
            "--timeout-seconds",
            "10",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(main.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(
            Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(&backend)
                .output()
                .unwrap()
                .stdout
        )
        .unwrap()
        .trim(),
        "main"
    );
    let shared: serde_json::Value =
        serde_json::from_slice(&fs::read(frontend.join(".nlab/nlab-api.config.json")).unwrap())
            .unwrap();
    assert_eq!(shared["backend"]["branch"], "main");
}
