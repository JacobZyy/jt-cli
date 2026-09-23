use super::*;
use rusqlite::Connection;

fn write(root: &Path, path: &str, source: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

fn index(root: &Path) -> Connection {
    fs::create_dir_all(root.join(".codegraph")).unwrap();
    let db = Connection::open(root.join(".codegraph/codegraph.db")).unwrap();
    db.execute_batch("CREATE TABLE project_metadata (key TEXT, value TEXT);
        INSERT INTO project_metadata VALUES ('index_state','complete'), ('indexed_with_version','test'), ('indexed_with_extraction_version','test');
        CREATE TABLE nodes (id TEXT PRIMARY KEY, kind TEXT, name TEXT, qualified_name TEXT, file_path TEXT, start_line INTEGER, start_column INTEGER DEFAULT 0, docstring TEXT, signature TEXT, decorators TEXT DEFAULT '', return_type TEXT DEFAULT '', language TEXT DEFAULT 'java');
        CREATE TABLE edges (source TEXT, target TEXT, kind TEXT, line INTEGER DEFAULT 0, col INTEGER DEFAULT 0, metadata TEXT, provenance TEXT);
        CREATE TABLE unresolved_refs (from_node_id TEXT, reference_name TEXT, reference_kind TEXT, line INTEGER, col INTEGER, file_path TEXT, status TEXT, language TEXT);").unwrap();
    db
}

fn add_source(
    root: &Path,
    db: &Connection,
    prefix: &str,
    owner: &str,
    interface: bool,
    source: &str,
) {
    let path = format!("{prefix}contract/{owner}.java");
    write(root, &path, source);
    let id = format!("{prefix}{owner}");
    let qualified = format!("p::{owner}");
    db.execute("INSERT INTO nodes (id,kind,name,qualified_name,file_path,start_line,signature) VALUES (?1,?2,?3,?4,?5,1,'')",
        rusqlite::params![id, if interface { "interface" } else { "class" }, owner, qualified, path]).unwrap();
    for (line, source) in source.lines().enumerate() {
        if source.contains("void run(") {
            let method = format!("{id}:run");
            let signature = if source.contains("EmployeeUser user") {
                "void (com.zhuanzhuan.arch.zgateway.support.EmployeeUser user)"
            } else {
                "void ()"
            };
            db.execute("INSERT INTO nodes (id,kind,name,qualified_name,file_path,start_line,signature) VALUES (?1,'method','run',?2,?3,?4,?5)",
                rusqlite::params![method, format!("{qualified}::run"), path, (line + 1) as i64, signature]).unwrap();
            db.execute(
                "INSERT INTO edges(source,target,kind) VALUES (?1,?2,'contains')",
                [&id, &method],
            )
            .unwrap();
        }
        if source.contains("Remote remote;") || source.contains("Missing missing;") {
            let (kind, name) = if source.contains("Remote remote;") {
                ("Remote", "remote")
            } else {
                ("Missing", "missing")
            };
            let field = format!("{id}:{name}");
            db.execute("INSERT INTO nodes (id,kind,name,qualified_name,file_path,start_line,signature) VALUES (?1,'field',?2,?3,?4,?5,?6)",
                rusqlite::params![field, name, format!("{qualified}::{name}"), path, (line + 1) as i64, format!("{kind} {name}")]).unwrap();
            db.execute(
                "INSERT INTO edges(source,target,kind) VALUES (?1,?2,'contains')",
                [&id, &field],
            )
            .unwrap();
        }
    }
}

fn fixture(unified: bool) -> (tempfile::TempDir, DiscoverArgs) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for name in ["a", "b"] {
        fs::create_dir_all(root.join(name).join(".git")).unwrap();
    }
    let mut config = crate::typescript::tests::config();
    config.backend.repo_path = root.join("a");
    config.backend.contract_roots = vec!["contract/Entry.java".to_owned()];
    write(
        root,
        "frontend/.nlab/nlab-api.config.json",
        &serde_json::to_string(&config).unwrap(),
    );
    let a = if unified {
        root.to_path_buf()
    } else {
        root.join("a")
    };
    let b = if unified {
        root.to_path_buf()
    } else {
        root.join("b")
    };
    let db = index(&a);
    let prefix = if unified { "a/" } else { "" };
    add_source(
        &a,
        &db,
        prefix,
        "Entry",
        true,
        "package p;\n@ServiceContract public interface Entry {\nvoid run(com.zhuanzhuan.arch.zgateway.support.EmployeeUser user);\n}\n",
    );
    add_source(
        &a,
        &db,
        prefix,
        "EntryImpl",
        false,
        "package p;\nimport p.Remote;\npublic class EntryImpl implements Entry {\nRemote remote;\nvoid run(com.zhuanzhuan.arch.zgateway.support.EmployeeUser user) { remote.run(); }\nvoid unused() { ignored.run(); }\n}\n",
    );
    db.execute(
        "INSERT INTO edges(source,target,kind) VALUES (?1,?2,'implements')",
        [format!("{prefix}EntryImpl"), format!("{prefix}Entry")],
    )
    .unwrap();
    let db = if unified { db } else { index(&b) };
    let prefix = if unified { "b/" } else { "" };
    add_source(
        &b,
        &db,
        prefix,
        "Remote",
        true,
        "package p;\npublic interface Remote {\nvoid run();\n}\n",
    );
    add_source(
        &b,
        &db,
        prefix,
        "RemoteImpl",
        false,
        "package p;\nimport p.Missing;\npublic class RemoteImpl implements Remote {\nMissing missing;\nvoid run() { missing.run(); }\n}\n",
    );
    db.execute(
        "INSERT INTO edges(source,target,kind) VALUES (?1,?2,'implements')",
        [format!("{prefix}RemoteImpl"), format!("{prefix}Remote")],
    )
    .unwrap();
    write(
        root,
        "a/service/src/main/resources/scf.xml",
        "<beans><zzscf:application applicationName='a'/><zzscf:references serviceName='b'><zzscf:reference interface='p.Remote'/></zzscf:references></beans>",
    );
    write(
        root,
        "b/service/src/main/resources/scf.xml",
        "<beans><zzscf:application applicationName='b'/><zzscf:references serviceName='missing'><zzscf:reference interface='p.Missing'/></zzscf:references></beans>",
    );
    let args = DiscoverArgs {
        project: root.join("frontend"),
        repositories_root: Some(root.to_path_buf()),
        service_branch: Vec::new(),
        entry: None,
        max_depth: 3,
        offline: true,
    };
    (temp, args)
}

#[test]
fn default_discovery_follows_sibling_repositories_without_configuration() {
    let (temp, mut args) = fixture(false);
    args.repositories_root = None;
    assert!(
        ProjectConfig::load(&args.project)
            .unwrap()
            .discovery
            .is_none()
    );
    let mut synced = Vec::new();
    let result = acquisition::scan_with(
        args.clone(),
        Some(&entry_routes()),
        |path| {
            synced.push(path.to_owned());
            Ok(())
        },
        |_, _, _, _| panic!("offline discovery must not acquire repositories"),
    )
    .unwrap();
    let result = finish_preflight(&args.project, result).unwrap();
    assert_eq!(
        result.report.repositories_root,
        temp.path().canonicalize().unwrap()
    );
    assert_eq!(result.report.calls[0].status, "source-matched");
    assert!(synced.iter().any(|path| path.ends_with("b")));
    assert_eq!(result.report.unavailable_services, ["missing"]);
    assert_eq!(
        ProjectConfig::load(&args.project)
            .unwrap()
            .discovery
            .unwrap()
            .services["b"]
            .status,
        "resolved"
    );
}

#[test]
fn discovery_drops_unconfigured_interface_methods() {
    let (_temp, args) = fixture(false);
    let error = scan(args, &BTreeMap::new(), &BTreeMap::new(), Some(&[]))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("no configured gateway routes matched")
    );
}

#[test]
fn discovery_root_prefers_explicit_then_saved_and_rejects_invalid_overrides() {
    let (temp, args) = fixture(false);
    let backend = temp.path().join("a").canonicalize().unwrap();
    let configured = temp.path().join("configured");
    let requested = temp.path().join("requested");
    fs::create_dir(&configured).unwrap();
    fs::create_dir(&requested).unwrap();
    save_root(&args.project, &configured).unwrap();
    assert_eq!(
        resolve_root(&args.project, None, &backend).unwrap(),
        configured.canonicalize().unwrap()
    );
    assert_eq!(
        resolve_root(&args.project, Some(&requested), &backend).unwrap(),
        requested.canonicalize().unwrap()
    );
    let missing = temp.path().join("missing");
    assert!(resolve_root(&args.project, Some(&missing), &backend).is_err());
    let file = temp.path().join("file");
    fs::write(&file, "not a directory").unwrap();
    assert!(
        resolve_root(&args.project, Some(&file), &backend)
            .unwrap_err()
            .to_string()
            .contains("must be a directory")
    );
    fs::remove_dir(&configured).unwrap();
    assert!(resolve_root(&args.project, None, &backend).is_err());
}

#[test]
fn discovers_only_reachable_services_with_repository_indexes() {
    {
        let (temp, args) = fixture(false);
        let config = fs::read(args.project.join(".nlab/nlab-api.config.json")).unwrap();
        let report = discover(args.clone()).unwrap();
        assert_eq!(report.entries, ["p.Entry"]);
        assert_eq!(report.calls.len(), 2, "{report:#?}");
        assert_eq!(report.calls[0].status, "source-matched");
        assert_eq!(report.calls[0].call.interface, "p.Remote");
        assert_eq!(
            report.calls[0].call.chain,
            ["p.Entry.run", "p.EntryImpl.run"]
        );
        assert_eq!(report.calls[1].status, "missing-source");
        assert_eq!(report.calls[1].call.interface, "p.Missing");
        assert_eq!(report.calls[1].depth, 1);
        assert!(
            report
                .repositories
                .iter()
                .all(|repository| repository.visited)
        );
        assert_eq!(
            config,
            fs::read(args.project.join(".nlab/nlab-api.config.json")).unwrap()
        );
        assert!(!temp.path().join("missing").exists());
        assert!(!args.project.join(".nlab/nlab-api.local.json").exists());
        let repeat = discover(args.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::to_value(repeat).unwrap()
        );

        let mut limited = args;
        limited.max_depth = 0;
        let report = discover(limited).unwrap();
        assert_eq!(report.calls.len(), 1);
        assert_eq!(report.calls[0].status, "depth-limit");
    }
}

#[test]
fn conflicting_service_owners_and_missing_indexes_stay_unresolved() {
    let (temp, args) = fixture(false);
    let root = temp.path();
    fs::create_dir_all(root.join("duplicate/.git")).unwrap();
    write(
        root,
        "duplicate/service/src/main/resources/scf.xml",
        "<zzscf:application applicationName='b'/>",
    );
    let report = discover(args.clone()).unwrap();
    assert_eq!(report.calls[0].status, "ambiguous");
    assert_eq!(report.calls[0].candidates.len(), 2);
    fs::remove_dir_all(root.join("duplicate")).unwrap();
    fs::remove_dir_all(root.join("b/.codegraph")).unwrap();
    let report = discover(args).unwrap();
    assert_eq!(report.calls[0].status, "index-unavailable");
    assert_eq!(report.repositories[1].index_status, "missing");
}

#[test]
fn scf_comments_do_not_create_bindings_and_credentials_are_not_reported() {
    let (services, bindings) = scf_bindings("<!-- <zzscf:application applicationName='wrong'/> -->\n<zzscf:application applicationName=\"actual\"/>\n<zzscf:references serviceName='first'>\n<zzscf:reference interface='p.A'/>\n</zzscf:references><zzscf:reference interface='p.B' serviceName='second'/>", "scf.xml").unwrap();
    assert_eq!(services, BTreeSet::from(["actual".to_owned()]));
    assert_eq!(bindings["p.A"][0].service, "first");
    assert_eq!(bindings["p.A"][0].line, 4);
    assert_eq!(bindings["p.B"][0].service, "second");
    assert_eq!(
        public_origin("https://user:secret@host/group/repo.git?token=secret"),
        "https://host/group/repo.git"
    );
}

#[test]
fn collection_index_is_never_used_and_entry_stays_in_backend() {
    let (_temp, args) = fixture(true);
    assert!(
        discover(args)
            .unwrap_err()
            .to_string()
            .contains("no usable CodeGraph")
    );
    let (_temp, mut args) = fixture(false);
    args.entry = Some(PathBuf::from("../b/contract/Remote.java"));
    assert!(
        discover(args)
            .unwrap_err()
            .to_string()
            .contains("inside configured backend")
    );
}

#[test]
fn overloaded_remote_method_is_not_guessed_and_unbound_candidate_is_not_followed() {
    let (temp, args) = fixture(false);
    let db = Connection::open(temp.path().join("b/.codegraph/codegraph.db")).unwrap();
    db.execute_batch(
        "INSERT INTO nodes (id,kind,name,qualified_name,file_path,start_line,signature)
        VALUES ('overload','method','run','p::Remote::run','contract/Remote.java',3,'void ()');
        INSERT INTO edges(source,target,kind) VALUES ('Remote','overload','contains');",
    )
    .unwrap();
    let report = discover(args.clone()).unwrap();
    assert_eq!(report.calls.len(), 1);
    assert_eq!(report.calls[0].status, "method-unresolved");
    write(
        temp.path(),
        "b/service/src/main/resources/scf.xml",
        "<beans/>",
    );
    let report = discover(args.clone()).unwrap();
    assert_eq!(report.calls[0].status, "unbound-candidate");
    assert_eq!(report.calls[0].candidates.len(), 1);
    write(
        temp.path(),
        "a/service/src/main/resources/scf.xml",
        "<zzscf:application applicationName='a'/>",
    );
    let report = discover(args).unwrap();
    assert_eq!(report.calls[0].status, "unbound-candidate");
    assert!(!report.repositories[1].visited);
}

#[test]
fn preflight_blocks_before_overwriting_existing_generated_files() {
    let (temp, args) = fixture(false);
    fs::remove_dir_all(temp.path().join("b/.codegraph")).unwrap();
    write(&args.project, ".nlab/contract-ir.json", "existing contract");
    let result = scan(
        args.clone(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&entry_routes()),
    )
    .unwrap();
    let error = finish_preflight(&args.project, result).err().unwrap();
    assert!(error.downcast_ref::<Blocked>().is_some());
    assert_eq!(
        fs::read_to_string(args.project.join(".nlab/contract-ir.json")).unwrap(),
        "existing contract"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(args.project.join(".nlab/generate-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["stages"]["discovery"]["blockingServices"][0], "b");
}

#[test]
fn acquisition_retries_legacy_waivers_and_keeps_requested_branch() {
    let (temp, mut args) = fixture(false);
    args.offline = false;
    let project = args.project.clone();
    let config_path = project.join(crate::config::CONFIG_FILE);
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["discovery"] = serde_json::json!({"services":{"b":{"branch":"master"},"missing":{"allowMissing":true,"status":"missing"}}});
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    save_branches(&project, &["missing=feature/test".to_owned()]).unwrap();
    for _ in 0..2 {
        let mut attempts = Vec::new();
        let result = acquisition::scan_with(
            args.clone(),
            Some(&entry_routes()),
            |_| Ok(()),
            |service, branch, existing, _| {
                attempts.push((service.to_owned(), branch.to_owned()));
                acquisition::Acquisition {
                    repository: Some(format!("git@example:{service}.git")),
                    branch: branch.to_owned(),
                    path: existing.map(Path::to_owned),
                    status: if service == "b" {
                        "updated"
                    } else {
                        "acquisition-failed"
                    }
                    .to_owned(),
                    error: (service == "missing")
                        .then(|| "Permission denied (publickey).".to_owned()),
                }
            },
        )
        .unwrap();
        let result = finish_preflight(&project, result).unwrap();
        assert_eq!(
            attempts,
            [
                ("b".to_owned(), "master".to_owned()),
                ("missing".to_owned(), "feature/test".to_owned())
            ]
        );
        assert_eq!(result.report.unavailable_services, ["missing"]);
        assert!(result.report.blocking_services.is_empty());
        let state = ProjectConfig::load(&project).unwrap().discovery.unwrap();
        assert_eq!(state.services["missing"].status, "acquisition-failed");
        assert_eq!(
            state.services["missing"].branch.as_deref(),
            Some("feature/test")
        );
        assert!(
            !fs::read_to_string(&config_path)
                .unwrap()
                .contains("allowMissing")
        );
    }
    args.offline = true;
    acquisition::scan_with(
        args,
        Some(&entry_routes()),
        |_| Ok(()),
        |_, _, _, _| panic!("offline must not acquire"),
    )
    .unwrap();
    assert!(!temp.path().join("missing").exists());
}

#[test]
fn acquisition_syncs_new_repository_and_failed_updates_never_use_stale_index() {
    let (temp, mut args) = fixture(false);
    args.offline = false;
    let saved = tempfile::tempdir().unwrap();
    fs::rename(temp.path().join("b"), saved.path().join("b")).unwrap();
    let mut synced = Vec::new();
    let result = acquisition::scan_with(
        args.clone(),
        Some(&entry_routes()),
        |path| {
            synced.push(path.to_owned());
            Ok(())
        },
        |service, branch, _, root| {
            let path = root.join(service);
            if service == "b" {
                fs::rename(saved.path().join("b"), &path).unwrap();
            }
            acquisition::Acquisition {
                repository: None,
                branch: branch.to_owned(),
                path: Some(path),
                status: if service == "b" {
                    "cloned"
                } else {
                    "acquisition-failed"
                }
                .to_owned(),
                error: None,
            }
        },
    )
    .unwrap();
    assert!(synced.iter().any(|path| path.ends_with("b")));
    assert_eq!(result.report.calls[0].status, "source-matched");
    let result = acquisition::scan_with(
        args.clone(),
        Some(&entry_routes()),
        |_| Ok(()),
        |_, branch, path, _| acquisition::Acquisition {
            repository: None,
            branch: branch.to_owned(),
            path: path.map(Path::to_owned),
            status: "acquisition-failed".to_owned(),
            error: Some("fetch denied".to_owned()),
        },
    )
    .unwrap();
    assert_eq!(result.report.calls.len(), 1);
    assert_eq!(result.report.calls[0].status, "index-unavailable");
    assert!(
        !result
            .report
            .repositories
            .iter()
            .find(|repository| repository.path.ends_with("b"))
            .unwrap()
            .visited
    );
    let result = finish_preflight(&args.project, result).unwrap();
    assert_eq!(result.report.unavailable_services, ["b"]);
    assert!(result.report.blocking_services.is_empty());
}

#[test]
fn sic_association_resolves_repositories_without_scf_application_xml() {
    let (temp, mut args) = fixture(false);
    args.offline = false;
    write(
        temp.path(),
        "b/service/src/main/resources/scf.xml",
        "<beans/>",
    );
    let result = acquisition::scan_with(
        args,
        Some(&entry_routes()),
        |_| Ok(()),
        |service, branch, _, root| {
            assert_eq!(service, "b");
            acquisition::Acquisition {
                repository: Some("git@example:b.git".to_owned()),
                branch: branch.to_owned(),
                path: Some(root.join("b")),
                status: "updated".to_owned(),
                error: None,
            }
        },
    )
    .unwrap();
    assert_eq!(result.report.calls[0].status, "source-matched");
    assert!(
        result
            .report
            .repositories
            .iter()
            .any(|repository| repository.path.ends_with("b") && repository.visited)
    );
}
