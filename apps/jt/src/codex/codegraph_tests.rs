use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::{TempDir, tempdir};

use super::render;

const REBUILD_MESSAGE: &str = "CodeGraph: source snapshot unavailable; rebuilding the local index.";

struct Fixture {
    directory: TempDir,
    primary: PathBuf,
    worktree: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let primary = directory.path().join("primary ' project");
        let worktree = directory.path().join("worktree ' project");
        fs::create_dir(&primary).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
            vec!["worktree", "add", "--detach", worktree.to_str().unwrap()],
        ] {
            let output = Command::new("git")
                .args(args)
                .current_dir(&primary)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        fs::create_dir(primary.join(".codegraph")).unwrap();
        rusqlite::Connection::open(primary.join(".codegraph/codegraph.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE evidence (value TEXT); INSERT INTO evidence VALUES ('original');",
            )
            .unwrap();
        let bin = directory.path().join("bin");
        fs::create_dir(&bin).unwrap();
        write_executable(
            &bin.join("codegraph"),
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> codegraph-actions\nexit \"${JT_TEST_CODEGRAPH_EXIT:-0}\"\n",
        );
        write_executable(
            &bin.join("sqlite3"),
            r#"#!/bin/sh
set -eu
if [ "$1" = '-readonly' ]; then
  printf '%s\n' "$5" >> "$JT_TEST_SQLITE_CALLS"
  destination="${5#.backup }"
  if [ "${JT_TEST_BACKUP_EXIT:-0}" != 0 ]; then
    printf '%s\n' 'partial database' > "$destination"
    printf '%s\n' 'sqlite backup fixture failure' >&2
    exit "$JT_TEST_BACKUP_EXIT"
  fi
  cp "$2" "$destination"
else
  printf '%s\n' 'normalize' >> "$JT_TEST_SQLITE_CALLS"
  exit "${JT_TEST_NORMALIZE_EXIT:-0}"
fi
"#,
        );
        let cache = primary.join(".git/jt-codex");
        Self {
            directory,
            primary,
            worktree,
            cache,
        }
    }

    fn setup(&self) -> Command {
        let content = render(&self.primary).unwrap();
        let script = content
            .split_once("script = '''\n")
            .unwrap()
            .1
            .split_once("'''")
            .unwrap()
            .0;
        let mut command = Command::new("sh");
        command
            .args(["-c", script])
            .current_dir(&self.worktree)
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(self.directory.path().join("bin"))
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            )
            .env(
                "JT_TEST_SQLITE_CALLS",
                self.directory.path().join("sqlite-calls"),
            );
        command
    }

    fn hold_cache_lock(&self) {
        fs::create_dir_all(self.cache.join("snapshot.lock")).unwrap();
        fs::write(self.cache.join("snapshot.lock/owner"), "another setup").unwrap();
    }

    fn assert_cleanup(&self, lock_held_elsewhere: bool) {
        if self.worktree.join(".codegraph").is_dir() {
            for entry in fs::read_dir(self.worktree.join(".codegraph")).unwrap() {
                assert!(
                    !entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("jt-copy.")
                );
            }
        }
        if self.cache.is_dir() {
            for entry in fs::read_dir(&self.cache).unwrap() {
                let name = entry.unwrap().file_name();
                let name = name.to_string_lossy();
                assert!(!name.starts_with("snapshot.") || name == "snapshot.lock");
            }
        }
        if lock_held_elsewhere {
            assert_eq!(
                fs::read_to_string(self.cache.join("snapshot.lock/owner")).unwrap(),
                "another setup"
            );
        } else {
            assert!(!self.cache.join("snapshot.lock").exists());
        }
    }

    fn actions(&self) -> String {
        fs::read_to_string(self.worktree.join("codegraph-actions")).unwrap_or_default()
    }
}

fn write_executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn backup_success_publishes_an_independent_database_in_both_lock_branches() {
    for lock_held_elsewhere in [false, true] {
        let fixture = Fixture::new();
        if lock_held_elsewhere {
            fixture.hold_cache_lock();
        }
        let source = fixture.primary.join(".codegraph/codegraph.db");
        let original = fs::read(&source).unwrap();
        let output = fixture.setup().output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fixture.actions(), "sync\n");
        let destination = fixture.worktree.join(".codegraph/codegraph.db");
        assert_eq!(fs::read(&destination).unwrap(), original);
        assert_ne!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&destination).unwrap().ino()
        );
        assert_eq!(fs::read(source).unwrap(), original);
        assert_eq!(
            fixture.cache.join("codegraph.db").exists(),
            !lock_held_elsewhere
        );
        let backup_name = if lock_held_elsewhere {
            "codegraph.db"
        } else {
            "snapshot.db"
        };
        assert_eq!(
            fs::read_to_string(fixture.directory.path().join("sqlite-calls")).unwrap(),
            format!(".backup {backup_name}\nnormalize\n")
        );
        if !lock_held_elsewhere {
            assert_ne!(
                fs::metadata(fixture.cache.join("codegraph.db"))
                    .unwrap()
                    .ino(),
                fs::metadata(&destination).unwrap().ino()
            );
        }
        assert!(!stderr(&output).contains(REBUILD_MESSAGE));
        fixture.assert_cleanup(lock_held_elsewhere);
    }
}

#[test]
fn backup_failure_rebuilds_and_removes_partial_snapshots_in_both_lock_branches() {
    for lock_held_elsewhere in [false, true] {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.cache).unwrap();
        fs::write(fixture.cache.join("codegraph.db"), "previous snapshot").unwrap();
        fs::write(fixture.cache.join("source-signature"), "stale signature").unwrap();
        if lock_held_elsewhere {
            fixture.hold_cache_lock();
        }
        let source = fixture.primary.join(".codegraph/codegraph.db");
        let original = fs::read(&source).unwrap();
        let output = fixture
            .setup()
            .env("JT_TEST_BACKUP_EXIT", "23")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fixture.actions(), "init\n");
        assert!(stderr(&output).contains("sqlite backup fixture failure"));
        assert!(stderr(&output).contains(REBUILD_MESSAGE));
        let backup_name = if lock_held_elsewhere {
            "codegraph.db"
        } else {
            "snapshot.db"
        };
        assert_eq!(
            fs::read_to_string(fixture.directory.path().join("sqlite-calls")).unwrap(),
            format!(".backup {backup_name}\n")
        );
        assert!(!fixture.worktree.join(".codegraph/codegraph.db").exists());
        assert_eq!(
            fs::read_to_string(fixture.cache.join("codegraph.db")).unwrap(),
            "previous snapshot"
        );
        assert_eq!(
            fs::read_to_string(fixture.cache.join("source-signature")).unwrap(),
            "stale signature"
        );
        assert_eq!(fs::read(source).unwrap(), original);
        fixture.assert_cleanup(lock_held_elsewhere);
    }
}

#[test]
fn normalization_failure_propagates_without_rebuilding_or_publishing() {
    for lock_held_elsewhere in [false, true] {
        let fixture = Fixture::new();
        if lock_held_elsewhere {
            fixture.hold_cache_lock();
        }
        let output = fixture
            .setup()
            .env("JT_TEST_NORMALIZE_EXIT", "41")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(41), "{output:?}");
        assert!(fixture.actions().is_empty());
        assert!(!stderr(&output).contains(REBUILD_MESSAGE));
        assert!(!fixture.worktree.join(".codegraph/codegraph.db").exists());
        assert!(!fixture.cache.join("codegraph.db").exists());
        fixture.assert_cleanup(lock_held_elsewhere);
    }
}

#[test]
fn init_and_sync_failures_remain_setup_failures() {
    for backup_fails in [false, true] {
        let fixture = Fixture::new();
        let output = fixture
            .setup()
            .env("JT_TEST_BACKUP_EXIT", if backup_fails { "23" } else { "0" })
            .env("JT_TEST_CODEGRAPH_EXIT", "19")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(19), "{output:?}");
        assert_eq!(
            fixture.actions(),
            if backup_fails { "init\n" } else { "sync\n" }
        );
        fixture.assert_cleanup(false);
    }
}

#[test]
fn snapshot_fallback_does_not_bypass_symlink_or_hard_link_rejection() {
    for unsafe_path in ["directory", "database", "hard-link", "source-wal", "cache"] {
        let fixture = Fixture::new();
        let source = fixture.primary.join(".codegraph/codegraph.db");
        if unsafe_path == "directory" {
            symlink(
                fixture.primary.join(".codegraph"),
                fixture.worktree.join(".codegraph"),
            )
            .unwrap();
        } else if unsafe_path == "cache" {
            symlink(fixture.directory.path(), &fixture.cache).unwrap();
        } else if unsafe_path == "source-wal" {
            symlink(
                fixture.directory.path().join("outside"),
                fixture.primary.join(".codegraph/codegraph.db-wal"),
            )
            .unwrap();
        } else {
            fs::create_dir(fixture.worktree.join(".codegraph")).unwrap();
            let destination = fixture.worktree.join(".codegraph/codegraph.db");
            if unsafe_path == "hard-link" {
                fs::hard_link(source, destination).unwrap();
            } else {
                symlink(source, destination).unwrap();
            }
        }
        let output = fixture
            .setup()
            .env("JT_TEST_BACKUP_EXIT", "23")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{unsafe_path}: {output:?}");
        assert!(
            stderr(&output).contains("Refusing"),
            "{unsafe_path}: {output:?}"
        );
        assert!(fixture.actions().is_empty());
        assert!(!stderr(&output).contains(REBUILD_MESSAGE));
    }
}

#[test]
fn unindexed_project_never_runs_codegraph() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.primary.join(".codegraph/codegraph.db")).unwrap();
    let output = fixture.setup().output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(fixture.actions().is_empty());
    assert!(!fixture.worktree.join(".codegraph").exists());
    assert!(!fixture.cache.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn system_sqlite_handles_wal_without_sidecars_without_changing_source() {
    let fixture = Fixture::new();
    let source = fixture.primary.join(".codegraph/codegraph.db");
    rusqlite::Connection::open(&source)
        .unwrap()
        .execute_batch("PRAGMA journal_mode=WAL; INSERT INTO evidence VALUES ('checkpointed');")
        .unwrap();
    let original = fs::read(&source).unwrap();
    assert_eq!(&original[18..20], &[2, 2]);
    for suffix in ["-wal", "-shm"] {
        assert!(
            !source
                .with_file_name(format!("codegraph.db{suffix}"))
                .exists()
        );
    }
    let probe_source = fixture.directory.path().join("probe.db");
    fs::write(&probe_source, &original).unwrap();
    let probe = Command::new("/usr/bin/sqlite3")
        .arg("-readonly")
        .arg(&probe_source)
        .args(["-cmd", ".timeout 10000", ".backup probe-snapshot.db"])
        .current_dir(fixture.directory.path())
        .output()
        .unwrap();
    write_executable(
        &fixture.directory.path().join("bin/sqlite3"),
        "#!/bin/sh\nexec /usr/bin/sqlite3 \"$@\"\n",
    );
    let output = fixture.setup().output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read(&source).unwrap(), original);
    if probe.status.success() {
        assert_eq!(fixture.actions(), "sync\n");
        assert!(!stderr(&output).contains(REBUILD_MESSAGE));
        let database =
            rusqlite::Connection::open(fixture.worktree.join(".codegraph/codegraph.db")).unwrap();
        assert_eq!(
            database
                .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    } else {
        assert!(
            stderr(&probe).contains("Error: unable to open database file"),
            "{probe:?}"
        );
        assert_eq!(fixture.actions(), "init\n");
        assert!(
            stderr(&output).contains("Error: unable to open database file"),
            "{output:?}"
        );
        assert!(stderr(&output).contains(REBUILD_MESSAGE));
        for suffix in ["-wal", "-shm"] {
            assert!(
                !source
                    .with_file_name(format!("codegraph.db{suffix}"))
                    .exists()
            );
        }
        assert!(!fixture.worktree.join(".codegraph/codegraph.db").exists());
        assert!(!fixture.cache.join("codegraph.db").exists());
    }
    fixture.assert_cleanup(false);
}
