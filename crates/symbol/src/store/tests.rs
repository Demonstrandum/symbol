use proptest::prelude::*;

use super::*;

fn test_connection(path: &Path) -> SqliteConnection {
    SqliteConnection::establish(&path.to_string_lossy()).unwrap()
}

fn schema_version(db: &mut SqliteConnection) -> i64 {
    database::migrations::schema_version(db).unwrap()
}

fn test_content_hash(label: &str) -> ContentHash {
    ContentHash::from(blake3::hash(label.as_bytes()))
}

fn test_tree_hash(label: &str) -> TreeHash {
    TreeHash::from(blake3::hash(label.as_bytes()))
}

fn blob_quarantine_path(dir: &Path, hash: &str) -> PathBuf {
    let hex = hash.strip_prefix("blake3:").unwrap_or(hash);
    dir.join("blobs/.quarantine")
        .join(&hex[..2])
        .join(&hex[2..])
}

struct TestClock {
    millis: AtomicU64,
}

#[derive(Default)]
struct ReferenceAliases {
    files: HashSet<String>,
    aliases: BTreeMap<String, String>,
}

impl ReferenceAliases {
    fn resolves_file(&self, path: &str) -> bool {
        let mut current = path.to_string();
        let mut visited = HashSet::new();
        for _ in 0..MAX_ALIAS_HOPS {
            if self.files.contains(&current) {
                return true;
            }
            if !visited.insert(current.clone()) {
                return false;
            }
            let Some(target) = self.aliases.get(&current) else {
                return false;
            };
            current.clone_from(target);
        }
        false
    }
}

impl TestClock {
    fn new(millis: u64) -> Self {
        Self {
            millis: AtomicU64::new(millis),
        }
    }

    fn advance(&self, millis: u64) {
        self.millis.fetch_add(millis, Ordering::Relaxed);
    }
}

impl Clock for TestClock {
    fn now_millis(&self) -> i64 {
        i64::try_from(self.millis.load(Ordering::Relaxed)).expect("test time fits in i64")
    }
}

#[test]
#[expect(clippy::cognitive_complexity)]
fn aliases_resolve_files_chains_directories_and_dangling_targets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("alias-test", "assets/app.js", b"app")
        .unwrap();

    let first = store
        .put_alias(
            "alias-test",
            "latest.js",
            "assets/app.js",
            FileMutationOptions::default(),
        )
        .unwrap();
    assert!(first.created);
    let direct = store.alias("alias-test", "latest.js").unwrap();
    assert_eq!(direct.resolved_kind, Some(AliasResolvedKind::File));
    assert_eq!(direct.resolved_size, Some(3));
    let Node::File { hash, .. } = store.lookup("alias-test", "latest.js").unwrap() else {
        panic!("file alias must resolve");
    };
    assert_eq!(Some(hash.to_wire()), direct.resolved_hash);

    store
        .put_aliases(
            "alias-test",
            &[
                AliasSpec {
                    path: "chain.js",
                    target: "latest.js",
                },
                AliasSpec {
                    path: "static",
                    target: "assets",
                },
                AliasSpec {
                    path: "missing",
                    target: "future/file.txt",
                },
            ],
            FileMutationOptions::default(),
        )
        .unwrap();
    assert!(matches!(
        store.lookup("alias-test", "chain.js"),
        Ok(Node::File { .. })
    ));
    assert!(matches!(
        store.lookup("alias-test", "static/app.js"),
        Ok(Node::File { .. })
    ));
    let static_listing = store.list_dir("alias-test", "static").unwrap();
    assert_eq!(static_listing.files, 1);
    assert_eq!(static_listing.entries[0].name, "app.js");
    assert!(matches!(
        store.lookup("alias-test", "missing"),
        Err(StoreError::NotFound)
    ));
    store
        .put_file("alias-test", "future/file.txt", b"future")
        .unwrap();
    assert!(matches!(
        store.lookup("alias-test", "missing"),
        Ok(Node::File { .. })
    ));
    let deleted = store.delete_file("alias-test", "future/file.txt").unwrap();
    assert_eq!(
        store.alias("alias-test", "missing").unwrap().resolved_kind,
        None
    );
    store
        .undo(
            "alias-test",
            deleted.undo.as_ref().map(|undo| undo.token.as_str()),
        )
        .unwrap();
    assert!(matches!(
        store.lookup("alias-test", "missing"),
        Ok(Node::File { .. })
    ));
    assert_eq!(store.aliases("alias-test").unwrap().len(), 4);
    let inventory = store.alias_inventory("alias-test").unwrap();
    assert_eq!(inventory.aliases.len(), 4);
    assert_eq!(
        store.alias_stats("alias-test").unwrap(),
        AliasStats {
            aliases: 4,
            resolved: 4,
            dangling: 0,
        }
    );
}

#[test]
fn directory_listings_include_nested_aliases_without_inflating_files_or_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("alias-list", "tree/file.txt", b"1234")
        .unwrap();
    let baseline = store.list_dir("alias-list", "").unwrap();
    store
        .put_alias(
            "alias-list",
            "tree/nested/link",
            "../file.txt",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .put_alias("alias-list", "view", "tree", FileMutationOptions::default())
        .unwrap();

    let root = store.list_dir("alias-list", "").unwrap();
    assert_eq!(root.alias_count, 2);
    assert_eq!(root.files, baseline.files);
    assert!(
        root.entries
            .iter()
            .all(|entry| entry.name != "view" && entry.name != "link")
    );
    let tree = store.list_dir("alias-list", "tree").unwrap();
    assert_eq!(tree.alias_count, 1);
    assert_eq!(tree.aliases[0].path, "tree/nested/link");
    assert_eq!(tree.files, 1);
    let view = store.list_dir("alias-list", "view").unwrap();
    assert_eq!(view.alias_count, 1);
    assert_eq!(view.aliases[0].path, "view/nested/link");
    assert_eq!(view.files, 1);
    assert_eq!(view.bytes, 4);
}

#[test]
fn alias_security_conflicts_cycles_no_follow_and_undo_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-safe", "real/file.txt", b"x").unwrap();
    assert!(matches!(
        store.put_alias(
            "alias-safe",
            "bad",
            "../../outside",
            FileMutationOptions::default()
        ),
        Err(StoreError::InvalidAliasTarget)
    ));
    assert!(matches!(
        store.put_alias(
            "alias-safe",
            "real/file.txt",
            "other",
            FileMutationOptions::default()
        ),
        Err(StoreError::AliasConflict)
    ));
    store
        .put_alias("alias-safe", "a", "b", FileMutationOptions::default())
        .unwrap();
    assert!(matches!(
        store.put_alias("alias-safe", "b", "a", FileMutationOptions::default()),
        Err(StoreError::AliasCycle)
    ));
    assert!(store.alias("alias-safe", "b").is_err());
    store
        .put_alias(
            "alias-safe",
            "linked",
            "real",
            FileMutationOptions::default(),
        )
        .unwrap();
    assert!(matches!(
        store.put_file("alias-safe", "linked/new.txt", b"no"),
        Err(StoreError::AliasWrite)
    ));
    let mutation = store
        .put_alias(
            "alias-safe",
            "temporary",
            "real/file.txt",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .undo(
            "alias-safe",
            mutation.undo.as_ref().map(|undo| undo.token.as_str()),
        )
        .unwrap();
    assert!(store.alias("alias-safe", "temporary").is_err());

    let chain = (0..65)
        .map(|index| {
            (
                format!("hop-{index:02}"),
                if index == 64 {
                    "real/file.txt".to_string()
                } else {
                    format!("hop-{:02}", index + 1)
                },
            )
        })
        .collect::<Vec<_>>();
    let specs = chain
        .iter()
        .map(|(path, target)| AliasSpec { path, target })
        .collect::<Vec<_>>();
    assert!(matches!(
        store.put_aliases("alias-safe", &specs, FileMutationOptions::default()),
        Err(StoreError::AliasHopLimit)
    ));
    assert!(store.alias("alias-safe", "hop-00").is_err());
}

#[test]
fn aliases_reject_containment_cycles_and_prefix_shadowing_in_both_orders() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-prefix", "seed.txt", b"x").unwrap();
    assert!(matches!(
        store.put_alias(
            "alias-prefix",
            "dir/link",
            ".",
            FileMutationOptions::default()
        ),
        Err(StoreError::AliasCycle)
    ));
    assert!(matches!(
        store.put_aliases(
            "alias-prefix",
            &[
                AliasSpec {
                    path: "root-link",
                    target: "dir",
                },
                AliasSpec {
                    path: "dir/back",
                    target: "../root-link",
                },
            ],
            FileMutationOptions::default(),
        ),
        Err(StoreError::AliasCycle)
    ));

    store
        .put_file("alias-prefix", "existing/child.txt", b"x")
        .unwrap();
    assert!(matches!(
        store.put_alias(
            "alias-prefix",
            "existing",
            "seed.txt",
            FileMutationOptions::default()
        ),
        Err(StoreError::AliasConflict)
    ));

    store
        .put_alias(
            "alias-prefix",
            "first",
            "seed.txt",
            FileMutationOptions::default(),
        )
        .unwrap();
    assert!(matches!(
        store.put_file("alias-prefix", "first/child.txt", b"x"),
        Err(StoreError::AliasWrite)
    ));
    assert!(matches!(
        store.put_alias(
            "alias-prefix",
            "first/child",
            "seed.txt",
            FileMutationOptions::default()
        ),
        Err(StoreError::AliasWrite)
    ));
}

#[test]
fn alias_targets_allow_safe_punctuation_but_reject_noise_controls_and_external_uris() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    for target in ["name:part", "query?part", "hash#part"] {
        store
            .put_file("alias-paths", target, target.as_bytes())
            .unwrap();
        store
            .put_alias(
                "alias-paths",
                &format!("link-{}", target.len()),
                target,
                FileMutationOptions::default(),
            )
            .unwrap();
    }
    for target in [
        "https://example.test/file",
        "mailto:user@example.test",
        "data:text/plain,x",
        "file:///tmp/x",
        ".DS_Store",
        "dir/\nname",
    ] {
        assert!(matches!(
            store.put_alias(
                "alias-paths",
                "rejected",
                target,
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidAliasTarget)
        ));
    }
    for path in ["bad\npath", "/absolute", r"back\slash", ".DS_Store"] {
        assert!(matches!(
            store.put_alias(
                "alias-paths",
                path,
                "name:part",
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidAliasTarget)
        ));
    }
}

#[test]
fn alias_retarget_reports_created_false() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-created", "one", b"1").unwrap();
    store.put_file("alias-created", "two", b"2").unwrap();
    store
        .put_alias(
            "alias-created",
            "link",
            "one",
            FileMutationOptions::default(),
        )
        .unwrap();
    let retargeted = store
        .put_alias(
            "alias-created",
            "link",
            "two",
            FileMutationOptions::default(),
        )
        .unwrap();
    assert!(!retargeted.created);
    assert!(retargeted.changed);
}

#[test]
fn alias_batch_database_failure_rolls_back_every_row_and_undo_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-failure", "target", b"x").unwrap();
    let undo_count = store.undo_stack("alias-failure").unwrap().entries.len();
    {
        let mut db = store.inner.writer.lock().unwrap();
        db.batch_execute(
            "CREATE TEMP TRIGGER fail_second_alias
             BEFORE INSERT ON aliases
             WHEN NEW.path = 'b'
             BEGIN
               SELECT RAISE(ABORT, 'injected alias failure');
             END;",
        )
        .unwrap();
    }
    assert!(
        store
            .put_aliases(
                "alias-failure",
                &[
                    AliasSpec {
                        path: "a",
                        target: "target",
                    },
                    AliasSpec {
                        path: "b",
                        target: "target",
                    },
                ],
                FileMutationOptions::default(),
            )
            .is_err()
    );
    assert!(store.aliases("alias-failure").unwrap().is_empty());
    assert_eq!(
        store.undo_stack("alias-failure").unwrap().entries.len(),
        undo_count
    );
}

#[test]
fn alias_inventory_identity_and_rows_share_one_reader_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-snapshot", "target", b"x").unwrap();
    store
        .put_alias(
            "alias-snapshot",
            "first",
            "target",
            FileMutationOptions::default(),
        )
        .unwrap();
    let initial = store.alias_inventory("alias-snapshot").unwrap();
    let (identity_tx, identity_rx) = std::sync::mpsc::sync_channel(0);
    let (continue_tx, continue_rx) = std::sync::mpsc::sync_channel(0);
    {
        let mut readers = store.inner.readers.available.lock().unwrap();
        let db = readers.last_mut().unwrap();
        let mut paused = false;
        db.set_instrumentation(move |event: diesel::connection::InstrumentationEvent<'_>| {
            if paused {
                return;
            }
            if let diesel::connection::InstrumentationEvent::FinishQuery {
                query,
                error: None,
                ..
            } = event
            {
                let sql = query.to_string();
                if sql.contains("content_revision") && sql.contains("sites") {
                    paused = true;
                    identity_tx.send(()).unwrap();
                    continue_rx.recv().unwrap();
                }
            }
        });
        drop(readers);
    }
    let reader_store = store.clone();
    let reader =
        std::thread::spawn(move || reader_store.alias_inventory("alias-snapshot").unwrap());
    identity_rx.recv().unwrap();
    store
        .put_alias(
            "alias-snapshot",
            "second",
            "target",
            FileMutationOptions::default(),
        )
        .unwrap();
    continue_tx.send(()).unwrap();
    let snapshot = reader.join().unwrap();
    assert_eq!(snapshot.content_revision, initial.content_revision);
    assert_eq!(snapshot.aliases, initial.aliases);
    assert_eq!(store.aliases("alias-snapshot").unwrap().len(), 2);
}

#[test]
fn alias_archives_export_relative_symlink_metadata_for_tar_and_zip() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store")).unwrap();
    store
        .put_file("alias-archive", "assets/app.js", b"app")
        .unwrap();
    store
        .put_alias(
            "alias-archive",
            "current/app.js",
            "../assets/app.js",
            FileMutationOptions::default(),
        )
        .unwrap();
    for (format, kind, extension) in [
        (ArchiveFormat::Tar, Kind::Tar, "tar"),
        (ArchiveFormat::Zip, Kind::Zip, "zip"),
    ] {
        let archive = dir.path().join(format!("site.{extension}"));
        store
            .pack_site_to_path("alias-archive", format, &archive)
            .unwrap();
        let plan = crate::upload::plan_archive(&archive, kind).unwrap();
        assert!(plan.members.contains(&crate::upload::ArchiveMember::Alias {
            path: "current/app.js".to_string(),
            canonical_target: "assets/app.js".to_string(),
        }));
    }
}

#[test]
fn tar_alias_export_supports_gnu_long_link_targets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store")).unwrap();
    let target = format!("{}/file.txt", "long".repeat(30));
    store.put_file("alias-long", &target, b"x").unwrap();
    store
        .put_alias(
            "alias-long",
            "link",
            &target,
            FileMutationOptions::default(),
        )
        .unwrap();
    let archive = dir.path().join("long.tar");
    store
        .pack_site_to_path("alias-long", ArchiveFormat::Tar, &archive)
        .unwrap();
    let plan = crate::upload::plan_archive(&archive, Kind::Tar).unwrap();
    assert!(plan.members.contains(&crate::upload::ArchiveMember::Alias {
        path: "link".to_string(),
        canonical_target: target,
    }));
}

#[test]
fn copy_move_and_whole_site_undo_preserve_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-source", "file.txt", b"x").unwrap();
    store
        .put_alias(
            "alias-source",
            "link.txt",
            "file.txt",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .copy_site("alias-source", Some("alias-copy"), None)
        .unwrap();
    assert!(matches!(
        store.lookup("alias-copy", "link.txt"),
        Ok(Node::File { .. })
    ));
    store.move_site("alias-copy", "alias-moved").unwrap();
    assert!(matches!(
        store.lookup("alias-moved", "link.txt"),
        Ok(Node::File { .. })
    ));
    store.undo("alias-moved", None).unwrap();
    assert!(matches!(
        store.lookup("alias-copy", "link.txt"),
        Ok(Node::File { .. })
    ));
    store.pop_site("alias-copy").unwrap();
    store.undo("alias-copy", None).unwrap();
    assert!(matches!(
        store.lookup("alias-copy", "link.txt"),
        Ok(Node::File { .. })
    ));
}

#[test]
fn alias_expiry_removes_only_the_alias_and_undo_restores_it() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store_clock = Arc::<TestClock>::clone(&clock);
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        store_clock,
    )
    .unwrap();
    store
        .put_file("alias-expiry", "dir/file.txt", b"x")
        .unwrap();
    store
        .put_alias(
            "alias-expiry",
            "linked",
            "dir",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "alias-expiry",
            "linked",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 1,
            }),
        )
        .unwrap();
    clock.advance(1_001);
    assert_eq!(store.sweep_expired().unwrap(), 1);
    assert!(store.alias("alias-expiry", "linked").is_err());
    assert!(matches!(
        store.lookup("alias-expiry", "dir/file.txt"),
        Ok(Node::File { .. })
    ));
    store.undo("alias-expiry", None).unwrap();
    assert!(matches!(
        store.lookup("alias-expiry", "linked/file.txt"),
        Ok(Node::File { .. })
    ));
}

#[test]
fn alias_expiry_is_zero_cost_dangling_safe_and_rejects_paths_below_directory_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::<TestClock>::clone(&clock),
    )
    .unwrap();
    store
        .put_file("alias-expiry-rules", "target/file.txt", b"1234")
        .unwrap();
    let baseline = store.list_sites().unwrap();
    store
        .put_alias(
            "alias-expiry-rules",
            "linked",
            "target",
            FileMutationOptions::default(),
        )
        .unwrap();
    let after_alias = store.list_sites().unwrap();
    assert_eq!(after_alias.files, baseline.files);
    assert_eq!(after_alias.bytes, baseline.bytes);
    assert!(matches!(
        store.set_expiry(
            "alias-expiry-rules",
            "linked/file.txt",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 60
            })
        ),
        Err(StoreError::AliasWrite)
    ));
    store
        .set_expiry(
            "alias-expiry-rules",
            "linked",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 60,
            }),
        )
        .unwrap();
    store
        .delete_file("alias-expiry-rules", "target/file.txt")
        .unwrap();
    assert_eq!(
        store
            .alias("alias-expiry-rules", "linked")
            .unwrap()
            .resolved_kind,
        None
    );
    assert!(store.expiry_report("alias-expiry-rules", "linked").is_ok());
    clock.advance(60_001);
    assert_eq!(store.sweep_expired().unwrap(), 1);
    assert!(store.alias("alias-expiry-rules", "linked").is_err());
}

#[test]
fn alias_retarget_and_dependency_changes_refresh_expiry_kind_size_and_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store_clock = Arc::<TestClock>::clone(&clock);
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        store_clock,
    )
    .unwrap();
    store.put_file("alias-policy", "small", b"x").unwrap();
    store
        .put_file("alias-policy", "folder/large", &[0_u8; 4096])
        .unwrap();
    store
        .put_alias(
            "alias-policy",
            "linked",
            "small",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "alias-policy",
            "linked",
            Some(ExpiryPolicy::Decay(DecayPolicy {
                min_age_seconds: 10,
                max_age_seconds: 1_000,
                max_size_bytes: 10_000,
                power: 1.0,
            })),
        )
        .unwrap();
    let site_id = {
        let mut db = store.inner.readers.get();
        site_id_locked(&mut db, "alias-policy").unwrap()
    };
    let before = {
        let mut db = store.inner.readers.get();
        load_expiry_policy_locked(&mut db, site_id, "linked")
            .unwrap()
            .unwrap()
    };
    clock.advance(1_000);
    store
        .put_alias(
            "alias-policy",
            "linked",
            "folder",
            FileMutationOptions::default(),
        )
        .unwrap();
    let after = {
        let mut db = store.inner.readers.get();
        load_expiry_policy_locked(&mut db, site_id, "linked")
            .unwrap()
            .unwrap()
    };
    assert_eq!(after.kind, ExpiryTargetKind::Folder);
    assert_eq!(after.size_bytes, 0);
    assert_ne!(after.own_deadline_millis, before.own_deadline_millis);

    clock.advance(1_000);
    store
        .put_file("alias-policy", "folder/another", &[0_u8; 4096])
        .unwrap();
    let dependency_changed = {
        let mut db = store.inner.readers.get();
        load_expiry_policy_locked(&mut db, site_id, "linked")
            .unwrap()
            .unwrap()
    };
    assert_eq!(dependency_changed.size_bytes, 0);
    assert_ne!(
        dependency_changed.own_deadline_millis,
        after.own_deadline_millis
    );
}

#[test]
fn intermediate_alias_retarget_refreshes_transitive_alias_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::<TestClock>::clone(&clock),
    )
    .unwrap();
    store.put_file("alias-transitive", "one", b"1").unwrap();
    store.put_file("alias-transitive", "two", b"22").unwrap();
    store
        .put_alias(
            "alias-transitive",
            "middle",
            "one",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .put_alias(
            "alias-transitive",
            "outer",
            "middle",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "alias-transitive",
            "outer",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 60,
            }),
        )
        .unwrap();
    let before = store.expiry_report("alias-transitive", "outer").unwrap();
    clock.advance(1_000);
    store
        .put_alias(
            "alias-transitive",
            "middle",
            "two",
            FileMutationOptions::default(),
        )
        .unwrap();
    let after = store.expiry_report("alias-transitive", "outer").unwrap();
    assert_ne!(after.refreshed_at, before.refreshed_at);
    assert_ne!(after.effective_expires_at, before.effective_expires_at);
}

#[test]
#[expect(clippy::too_many_lines)]
fn alias_expiry_caps_follow_targets_chains_directories_and_dangling_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::<TestClock>::clone(&clock),
    )
    .unwrap();
    store
        .put_file("alias-caps", "target.txt", b"target")
        .unwrap();
    store
        .put_file("alias-caps", "directory/item.txt", b"item")
        .unwrap();
    store
        .put_file("alias-caps", "links/anchor.txt", b"anchor")
        .unwrap();
    let target = store
        .set_expiry(
            "alias-caps",
            "target.txt",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 80,
            }),
        )
        .unwrap()
        .report;
    let directory_item = store
        .set_expiry(
            "alias-caps",
            "directory/item.txt",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 30,
            }),
        )
        .unwrap()
        .report;
    store
        .put_aliases(
            "alias-caps",
            &[
                AliasSpec {
                    path: "links/target-capped",
                    target: "../target.txt",
                },
                AliasSpec {
                    path: "links/direct",
                    target: "../target.txt",
                },
                AliasSpec {
                    path: "links/chain",
                    target: "direct",
                },
                AliasSpec {
                    path: "view",
                    target: "directory",
                },
                AliasSpec {
                    path: "dangling",
                    target: "missing",
                },
            ],
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "alias-caps",
            "links",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 120,
            }),
        )
        .unwrap();
    let direct = store
        .set_expiry(
            "alias-caps",
            "links/direct",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 40,
            }),
        )
        .unwrap()
        .report;
    store
        .set_expiry(
            "alias-caps",
            "links/chain",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 60,
            }),
        )
        .unwrap();
    let dangling = store
        .set_expiry(
            "alias-caps",
            "dangling",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 45,
            }),
        )
        .unwrap()
        .report;

    assert_eq!(
        store
            .expiry_report("alias-caps", "links/target-capped")
            .unwrap()
            .effective_expires_at,
        target.effective_expires_at
    );
    assert_eq!(
        store
            .expiry_report("alias-caps", "links/chain")
            .unwrap()
            .effective_expires_at,
        direct.effective_expires_at
    );
    assert_eq!(
        store
            .expiry_report("alias-caps", "view/item.txt")
            .unwrap()
            .effective_expires_at,
        directory_item.effective_expires_at
    );
    assert_eq!(
        store
            .expiry_report("alias-caps", "dangling")
            .unwrap()
            .effective_expires_at,
        dangling.effective_expires_at
    );
}

#[test]
fn unrelated_write_refreshes_only_affected_alias_closure() {
    const ALIAS_COUNT: usize = 4096;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("alias-refresh", "target.txt", b"target")
        .unwrap();
    let paths = (0..ALIAS_COUNT)
        .map(|index| format!("links/{index:04}.txt"))
        .collect::<Vec<_>>();
    let specs = paths
        .iter()
        .map(|path| AliasSpec {
            path,
            target: "../target.txt",
        })
        .collect::<Vec<_>>();
    store
        .put_aliases("alias-refresh", &specs, FileMutationOptions::default())
        .unwrap();

    reset_alias_refresh_row_work();
    store
        .put_file("alias-refresh", "unrelated/new.txt", b"new")
        .unwrap();
    let work = alias_refresh_row_work();
    assert!(
        work.aliases <= 2,
        "unrelated write loaded {} of {ALIAS_COUNT} aliases",
        work.aliases
    );
    assert!(
        work.real <= 2,
        "unrelated write loaded {} unrelated real entries",
        work.real
    );
    assert!(matches!(
        store.lookup("alias-refresh", "links/4095.txt"),
        Ok(Node::File { .. })
    ));
}

#[test]
fn directory_alias_listing_row_work_is_prefix_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-bounded", "tree/a", b"a").unwrap();
    store.put_file("alias-bounded", "tree/b", b"b").unwrap();
    store
        .allocate_bytes(
            "alias-bounded",
            b"allocated",
            AllocationSpec {
                folder: "tree",
                ..AllocationSpec::default()
            },
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .put_file("alias-bounded", "elsewhere/a", b"a")
        .unwrap();
    store
        .put_file("alias-bounded", "elsewhere/b", b"b")
        .unwrap();
    for index in 0..256 {
        store
            .put_file("alias-bounded", &format!("unrelated/{index:03}"), b"x")
            .unwrap();
    }
    store
        .put_alias(
            "alias-bounded",
            "tree/nested",
            "a",
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .put_alias(
            "alias-bounded",
            "tree/external",
            "../elsewhere",
            FileMutationOptions::default(),
        )
        .unwrap();
    let unrelated_paths = (0..256)
        .map(|index| format!("unrelated-links/{index:03}"))
        .collect::<Vec<_>>();
    let unrelated = unrelated_paths
        .iter()
        .map(|path| AliasSpec {
            path,
            target: "../unrelated/000",
        })
        .collect::<Vec<_>>();
    store
        .put_aliases("alias-bounded", &unrelated, FileMutationOptions::default())
        .unwrap();
    store
        .put_alias(
            "alias-bounded",
            "view",
            "tree",
            FileMutationOptions::default(),
        )
        .unwrap();

    reset_alias_directory_row_work();
    let listing = store.list_dir("alias-bounded", "view").unwrap();
    let work = alias_directory_row_work();
    assert_eq!(listing.files, 3);
    assert_eq!(listing.alias_count, 2);
    assert_eq!(work.files, 2);
    assert_eq!(work.allocated, 1);
    assert_eq!(work.aliases, 2);
    assert_eq!(work.aggregates, 1);
    assert!(work.resolution <= 12);
}

#[test]
fn alias_paths_reject_controls_and_noise_but_preserve_safe_punctuation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("alias-paths", "target.txt", b"target")
        .unwrap();
    for invalid in [
        "line\nbreak",
        "carriage\rreturn",
        "tab\tpath",
        "bell\u{7}path",
        ".DS_Store",
        "nested/Thumbs.db",
        "nested/._resource",
    ] {
        assert!(matches!(
            store.put_alias(
                "alias-paths",
                invalid,
                "target.txt",
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidAliasTarget)
        ));
    }

    let safe = "safe/quote\" ' []{}=+,;!@~$^&()#%?.txt";
    store
        .put_alias(
            "alias-paths",
            safe,
            "../target.txt",
            FileMutationOptions::default(),
        )
        .unwrap();
    assert!(matches!(
        store.lookup("alias-paths", safe),
        Ok(Node::File { .. })
    ));
    let Node::File { hash, .. } = store.lookup("alias-paths", MANIFEST_PATH).unwrap() else {
        panic!("manifest must be a file");
    };
    let manifest = String::from_utf8(store.read_blob(hash).unwrap().to_vec()).unwrap();
    let parsed = toml::from_str::<toml::Value>(&manifest).unwrap();
    assert_eq!(
        parsed["aliases"][safe].as_str(),
        Some("target.txt"),
        "safe punctuation must round-trip through generated TOML"
    );
}

#[test]
fn zip_safe_alias_target_limit_roundtrips_at_boundary_and_rejects_overflow() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store")).unwrap();
    let boundary = format!("{}aa", "a/".repeat((MAX_ALIAS_TARGET_BYTES - 2) / 2));
    assert_eq!(boundary.len(), MAX_ALIAS_TARGET_BYTES);
    store.put_file("alias-limit", "seed", b"x").unwrap();
    store
        .put_alias(
            "alias-limit",
            "link",
            &boundary,
            FileMutationOptions::default(),
        )
        .unwrap();
    let archive = dir.path().join("boundary.zip");
    store
        .pack_site_to_path("alias-limit", ArchiveFormat::Zip, &archive)
        .unwrap();
    let plan = crate::upload::plan_archive(&archive, Kind::Zip).unwrap();
    assert!(plan.members.contains(&crate::upload::ArchiveMember::Alias {
        path: "link".to_string(),
        canonical_target: boundary.clone(),
    }));

    let overflow = format!("{boundary}x");
    assert!(matches!(
        store.put_alias(
            "alias-limit",
            "too-long",
            &overflow,
            FileMutationOptions::default()
        ),
        Err(StoreError::InvalidAliasTarget)
    ));
}

#[test]
fn alias_batch_scales_to_42802_entries_atomically() {
    const ALIAS_COUNT: usize = 42_802;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-scale", "target.txt", b"x").unwrap();
    let paths = (0..ALIAS_COUNT)
        .map(|index| format!("links/{index:05}.txt"))
        .collect::<Vec<_>>();
    let specs = paths
        .iter()
        .map(|path| AliasSpec {
            path,
            target: "../target.txt",
        })
        .collect::<Vec<_>>();
    let mutation = store
        .put_aliases("alias-scale", &specs, FileMutationOptions::default())
        .unwrap();
    assert_eq!(mutation.files, ALIAS_COUNT);
    assert_eq!(
        store.alias_stats("alias-scale").unwrap().resolved,
        ALIAS_COUNT as u64
    );
    assert!(matches!(
        store.lookup("alias-scale", "links/42801.txt"),
        Ok(Node::File { .. })
    ));
    let alias_queries = Arc::new(AtomicU64::new(0));
    {
        let counter = Arc::clone(&alias_queries);
        let mut db = store.inner.writer.lock().unwrap();
        db.set_instrumentation(move |event: diesel::connection::InstrumentationEvent<'_>| {
            if let diesel::connection::InstrumentationEvent::StartQuery { query, .. } = event {
                let sql = query.to_string();
                if sql.contains("SELECT") && sql.contains("aliases") {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }
    let hash = ContentHash::from(blake3::hash(b"x"));
    let staged = (0..ALIAS_COUNT)
        .map(|index| StagedFile {
            path: format!("files/{index:05}.txt"),
            size: 1,
            hash,
            source: StagedSource::Bytes(vec![b'x']),
            sanitized: TokenCounts::default(),
        })
        .collect::<Vec<_>>();
    let file_mutation = store
        .merge_staged("alias-scale", &staged, UndoKind::Put)
        .unwrap();
    let no_op = store
        .put_aliases("alias-scale", &specs, FileMutationOptions::default())
        .unwrap();
    assert!(!no_op.changed);
    let bounded_alias_queries = alias_queries.load(Ordering::Relaxed);
    assert!(
        (1..=12).contains(&bounded_alias_queries),
        "alias query work must stay batch-bounded"
    );
    store
        .undo(
            "alias-scale",
            file_mutation.undo.as_ref().map(|undo| undo.token.as_str()),
        )
        .unwrap();
    store
        .undo(
            "alias-scale",
            mutation.undo.as_ref().map(|undo| undo.token.as_str()),
        )
        .unwrap();
    assert_eq!(store.alias_stats("alias-scale").unwrap().aliases, 0);
}

#[test]
fn alias_cache_refresh_writes_only_rows_whose_resolution_changed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-cache", "target", b"x").unwrap();
    let paths = (0..128)
        .map(|index| format!("links/{index:03}"))
        .collect::<Vec<_>>();
    let specs = paths
        .iter()
        .map(|path| AliasSpec {
            path,
            target: "../target",
        })
        .collect::<Vec<_>>();
    store
        .put_aliases("alias-cache", &specs, FileMutationOptions::default())
        .unwrap();
    {
        let mut db = store.inner.writer.lock().unwrap();
        db.batch_execute(
            "CREATE TEMP TABLE alias_update_count (count INTEGER NOT NULL);
             INSERT INTO alias_update_count VALUES (0);
             CREATE TEMP TRIGGER count_alias_cache_updates
             BEFORE UPDATE ON aliases
             BEGIN
               UPDATE alias_update_count SET count = count + 1;
             END;",
        )
        .unwrap();
    }
    store.put_file("alias-cache", "unrelated", b"u").unwrap();
    {
        let mut db = store.inner.writer.lock().unwrap();
        let count = diesel::sql_query("SELECT count FROM alias_update_count")
            .get_result::<AliasUpdateCount>(&mut *db)
            .unwrap()
            .count;
        assert_eq!(count, 0);
        diesel::sql_query("UPDATE alias_update_count SET count = 0")
            .execute(&mut *db)
            .unwrap();
    }
    store.put_file("alias-cache", "target", b"changed").unwrap();
    let mut db = store.inner.writer.lock().unwrap();
    let count = diesel::sql_query("SELECT count FROM alias_update_count")
        .get_result::<AliasUpdateCount>(&mut *db)
        .unwrap()
        .count;
    drop(db);
    assert_eq!(count, 128);
}

#[test]
fn deterministic_alias_sequences_match_the_simple_reference_model() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("alias-model", "seed.txt", b"seed").unwrap();
    let mut reference = ReferenceAliases::default();
    reference.files.insert("seed.txt".to_string());
    let mut state = 0x42_u64;
    for index in 0..256 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let path = format!("alias-{index:03}");
        let target = match state % 5 {
            0 => "seed.txt".to_string(),
            1 if index > 0 => format!(
                "alias-{:03}",
                usize::try_from(state % u64::try_from(index).expect("index fits in u64"))
                    .expect("bounded index fits in usize")
            ),
            _ => format!("missing-{:03}", state % 31),
        };
        store
            .put_alias(
                "alias-model",
                &path,
                &target,
                FileMutationOptions::default(),
            )
            .unwrap();
        reference.aliases.insert(path.clone(), target);
        if index > 0 && index % 11 == 0 {
            let retargeted = format!("alias-{:03}", index / 2);
            store
                .put_alias(
                    "alias-model",
                    &retargeted,
                    "seed.txt",
                    FileMutationOptions::default(),
                )
                .unwrap();
            reference.aliases.insert(retargeted, "seed.txt".to_string());
        }
        if index > 0 && index % 53 == 0 {
            let deleted = format!("alias-{:03}", index / 3);
            store.delete_file("alias-model", &deleted).unwrap();
            reference.aliases.remove(&deleted);
        }
        if index > 0 && index % 37 == 0 {
            store.delete_file("alias-model", "seed.txt").unwrap();
            reference.files.remove("seed.txt");
            for alias in reference.aliases.keys() {
                assert_eq!(
                    store.lookup("alias-model", alias).is_ok(),
                    reference.resolves_file(alias)
                );
            }
            store.put_file("alias-model", "seed.txt", b"seed").unwrap();
            reference.files.insert("seed.txt".to_string());
        }
        for alias in reference.aliases.keys() {
            assert_eq!(
                store.lookup("alias-model", alias).is_ok(),
                reference.resolves_file(alias)
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn generated_alias_sequences_match_the_reference_model(
        choices in prop::collection::vec((any::<u8>(), any::<bool>()), 1..96)
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-property", "seed.txt", b"seed").unwrap();
        let mut reference = ReferenceAliases::default();
        reference.files.insert("seed.txt".to_string());

        for (index, (choice, retarget)) in choices.into_iter().enumerate() {
            let path = format!("alias-{index:03}");
            let target = match choice % 3 {
                0 => "seed.txt".to_string(),
                1 if index > 0 => format!("alias-{:03}", usize::from(choice) % index),
                _ => format!("missing-{:03}", choice % 17),
            };
            store
                .put_alias(
                    "alias-property",
                    &path,
                    &target,
                    FileMutationOptions::default(),
                )
                .unwrap();
            reference.aliases.insert(path.clone(), target);

            if retarget && index > 0 {
                let changed = format!("alias-{:03}", usize::from(choice) % index);
                store
                    .put_alias(
                        "alias-property",
                        &changed,
                        "seed.txt",
                        FileMutationOptions::default(),
                    )
                    .unwrap();
                reference.aliases.insert(changed, "seed.txt".to_string());
            }

            for alias in reference.aliases.keys() {
                prop_assert_eq!(
                    store.lookup("alias-property", alias).is_ok(),
                    reference.resolves_file(alias),
                    "generated step {}, alias {}",
                    index,
                    alias,
                );
            }
        }
    }

    #[test]
    fn generated_control_characters_are_rejected_from_alias_paths(
        prefix in "[a-z]{0,12}",
        suffix in "[a-z]{0,12}",
        control in 0_u8..=31,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("alias-fuzz", "target", b"x").unwrap();
        let invalid = format!("{prefix}{}{suffix}", char::from(control));
        prop_assert!(store
            .put_alias(
                "alias-fuzz",
                &invalid,
                "target",
                FileMutationOptions::default(),
            )
            .is_err());
    }
}

#[test]
fn direct_alias_read_cost_ignores_unrelated_dangling_aliases() {
    const UNRELATED: usize = 5_000;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("alias-read-cost", "target", b"value")
        .unwrap();
    store
        .put_alias(
            "alias-read-cost",
            "direct",
            "target",
            FileMutationOptions::default(),
        )
        .unwrap();
    let paths = (0..UNRELATED)
        .map(|index| format!("noise/{index:04}"))
        .collect::<Vec<_>>();
    let targets = (0..UNRELATED)
        .map(|index| format!("../missing/{index:04}"))
        .collect::<Vec<_>>();
    let aliases = paths
        .iter()
        .zip(&targets)
        .map(|(path, target)| AliasSpec { path, target })
        .collect::<Vec<_>>();
    store
        .put_aliases("alias-read-cost", &aliases, FileMutationOptions::default())
        .unwrap();

    reset_alias_directory_row_work();
    assert!(matches!(
        store.lookup("alias-read-cost", "direct"),
        Ok(Node::File { .. })
    ));
    let work = alias_directory_row_work();
    assert!(
        work.resolution <= 1,
        "direct alias read inspected {} rows with {UNRELATED} unrelated aliases",
        work.resolution
    );
}

#[test]
fn migrations_merge_manifest_and_reserved_paths_follow_contract() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::with_public_url(
        dir.path().to_path_buf(),
        "https://symbol.example".to_string(),
    )
    .unwrap();
    store
        .replace_site("hello", b"first", Kind::Html, None, false)
        .unwrap();
    store
        .replace_site("hello", b"second", Kind::File, Some("other.txt"), false)
        .unwrap();

    assert!(matches!(
        store.lookup("hello", "index.html"),
        Ok(Node::File { .. })
    ));
    let Node::File { hash, .. } = store.lookup("hello", MANIFEST_PATH).unwrap() else {
        panic!("manifest must be stored");
    };
    let manifest = String::from_utf8(store.read_blob(hash).unwrap().to_vec()).unwrap();
    assert!(manifest.contains("host = \"https://symbol.example\""));
    assert!(manifest.contains("name = \"hello\""));
    assert!(manifest.contains("content_revision = 2"));
    assert!(manifest.contains("\"index.html\" = \"blake3:"));
    assert!(manifest.contains("\"other.txt\" = \"blake3:"));
    assert!(matches!(
        store.put_file("hello", "nested/UNDO", b"no"),
        Err(StoreError::Upload(UploadError::ReservedPath))
    ));
    let mut db = test_connection(&dir.path().join("symbol.db"));
    assert_eq!(
        schema_version(&mut db),
        database::schema::LATEST_SCHEMA_VERSION
    );
}

#[test]
fn production_v6_database_is_safely_baselined() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("symbol.db");
    let mut db = test_connection(&path);
    database::migrations::migrate(&mut db).unwrap();
    diesel::insert_into(sites::table)
        .values(NewSite {
            name: "existing",
            created: Some(1),
            updated: 1,
            public_url: "https://symbol.example",
            content_revision: 7,
            tree_hash: test_tree_hash("existing"),
            creator_kind: None,
            creator_hash: None,
            claim_hash: None,
            management_hash: None,
            management_status: 0,
        })
        .execute(&mut db)
        .unwrap();
    run_migrations(&mut db).unwrap();
    run_migrations(&mut db).unwrap();
    assert_eq!(
        schema_version(&mut db),
        database::schema::LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        sites::table
            .filter(sites::name.eq("existing"))
            .select(sites::content_revision)
            .first::<i64>(&mut db)
            .unwrap(),
        7
    );
}

#[test]
fn migration_integrity_gate_rejects_foreign_key_violations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("symbol.db");
    let mut db = test_connection(&path);
    run_migrations(&mut db).unwrap();
    db.batch_execute("PRAGMA foreign_keys = OFF").unwrap();
    diesel::insert_into(files::table)
        .values(NewFile {
            site_id: 999,
            path: "orphan.txt".to_string(),
            hash: test_content_hash("missing"),
            size: 1,
        })
        .execute(&mut db)
        .unwrap();
    db.batch_execute("PRAGMA foreign_keys = ON").unwrap();
    assert!(matches!(
        run_migrations(&mut db),
        Err(StoreError::Io(error))
            if error.to_string() == "2 foreign-key violations"
    ));
}

#[test]
fn production_shaped_v2_database_migrates_through_current_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("symbol.db");
    let mut db = test_connection(&path);
    database::migrations::migrate(&mut db).unwrap();
    diesel::insert_into(sites::table)
        .values(NewSite {
            name: "legacy",
            created: Some(1),
            updated: 1,
            public_url: "https://symbol.example",
            content_revision: 2,
            tree_hash: test_tree_hash("legacy"),
            creator_kind: None,
            creator_hash: None,
            claim_hash: None,
            management_hash: None,
            management_status: 0,
        })
        .execute(&mut db)
        .unwrap();
    let site_id = site_id_locked(&mut db, "legacy").unwrap();
    diesel::insert_into(blobs::table)
        .values((
            blobs::hash.eq(test_content_hash("legacy-hash")),
            blobs::bytes.eq(Vec::<u8>::new()),
            blobs::size.eq(4_i64),
        ))
        .execute(&mut db)
        .unwrap();
    ensure_file_entry(&mut db, site_id, "index.html").unwrap();
    diesel::insert_into(files::table)
        .values(NewFile {
            site_id,
            path: "index.html".to_string(),
            hash: test_content_hash("legacy-hash"),
            size: 4,
        })
        .execute(&mut db)
        .unwrap();
    database::migrations::downgrade_to_v2(&mut db).unwrap();

    run_migrations(&mut db).unwrap();

    assert_eq!(
        schema_version(&mut db),
        database::schema::LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        path_aggregates::table
            .find((site_id, ""))
            .select((path_aggregates::logical_bytes, path_aggregates::file_count))
            .first::<(i64, i64)>(&mut db)
            .unwrap(),
        (4, 1)
    );
    assert_eq!(
        management_audit::table
            .count()
            .get_result::<i64>(&mut db)
            .unwrap(),
        0
    );
}

#[test]
fn path_aggregates_follow_incremental_put_delete_copy_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "assets/a.txt", b"aaa").unwrap();
    store
        .put_file("hello", "assets/nested/b.txt", b"bb")
        .unwrap();
    store.put_file("hello", "root.txt", b"r").unwrap();
    store.put_file("hello", "assets/a.txt", b"aaaaa").unwrap();

    let aggregate = |site: &str, path: &str| {
        let mut db = test_connection(&dir.path().join("symbol.db"));
        let site_id = site_id_locked(&mut db, site).unwrap();
        path_aggregates::table
            .find((site_id, path))
            .select((path_aggregates::logical_bytes, path_aggregates::file_count))
            .first::<(i64, i64)>(&mut db)
            .unwrap()
    };
    assert_eq!(aggregate("hello", ""), (8, 3));
    assert_eq!(aggregate("hello", "assets"), (7, 2));
    assert_eq!(aggregate("hello", "assets/nested"), (2, 1));

    store.delete_file("hello", "assets/nested").unwrap();
    assert_eq!(aggregate("hello", ""), (6, 2));
    assert_eq!(aggregate("hello", "assets"), (5, 1));

    store.copy_site("hello", Some("copy"), None).unwrap();
    assert_eq!(aggregate("copy", ""), (6, 2));
    let token = store.undo_stack("hello").unwrap().entries[0].token.clone();
    store.undo("hello", Some(&token)).unwrap();
    assert_eq!(aggregate("hello", ""), (8, 3));
    assert_eq!(aggregate("hello", "assets/nested"), (2, 1));
}

#[test]
fn undo_is_guarded_bounded_and_keeps_blobs_until_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::<TestClock>::clone(&clock),
    )
    .unwrap();
    store.put_file("hello", "index.html", b"first").unwrap();
    let first_hash = match store.lookup("hello", "index.html").unwrap() {
        Node::File { hash, .. } => hash,
        Node::Dir => panic!("expected file"),
    };
    clock.advance(1);
    store.put_file("hello", "index.html", b"second").unwrap();
    let stack = store.undo_stack("hello").unwrap();
    assert_eq!(stack.entries.len(), 2);
    assert!(matches!(
        store.undo("hello", Some(&stack.entries[1].token)),
        Err(StoreError::StaleUndo(_))
    ));
    store.undo("hello", Some(&stack.entries[0].token)).unwrap();
    let Node::File { hash, .. } = store.lookup("hello", "index.html").unwrap() else {
        panic!("expected restored file");
    };
    assert_eq!(hash, first_hash);

    for index in 0..12 {
        clock.advance(1);
        store
            .put_file("hello", &format!("{index}.txt"), b"value")
            .unwrap();
    }
    assert_eq!(store.undo_stack("hello").unwrap().entries.len(), 10);

    clock.advance(u64::try_from(UNDO_RETENTION_MILLIS).unwrap() + 1);
    store.prune_undo_and_gc().unwrap();
    assert!(store.undo_stack("hello").unwrap().entries.is_empty());
}

#[test]
fn garbage_collection_handles_more_dead_blobs_than_sqlite_variable_limit() {
    const DEAD_BLOBS: usize = 40_000;

    let dir = tempfile::tempdir().unwrap();
    let _store = Store::new(dir.path().to_path_buf()).unwrap();
    let mut db = test_connection(&dir.path().join("symbol.db"));
    let mut tx = DbTransaction::begin(&mut db).unwrap();
    for index in 0..DEAD_BLOBS {
        let hash = ContentHash::parse_hex(&format!("{index:064x}")).unwrap();
        diesel::insert_into(blobs::table)
            .values((
                blobs::hash.eq(hash),
                blobs::bytes.eq(Vec::<u8>::new()),
                blobs::size.eq(1_i64),
            ))
            .execute(&mut *tx)
            .unwrap();
    }

    let removed = gc_blobs(&mut tx, 0).unwrap();
    assert_eq!(removed.len(), DEAD_BLOBS);
    assert_eq!(blobs::table.count().get_result::<i64>(&mut *tx).unwrap(), 0);
    tx.commit().unwrap();
}

#[test]
fn pagefind_scale_chunked_sync_remains_writable_after_undo_rotation() {
    const FILES: usize = 42_802;
    const CHUNK_FILES: usize = 4_000;
    const FINAL_DELTA: usize = 1_900;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    for start in (0..FILES).step_by(CHUNK_FILES) {
        let end = (start + CHUNK_FILES).min(FILES);
        let staged = (start..end)
            .map(|index| {
                stage_bytes(
                    &format!("pagefind/fragment/{index:05}.pf_fragment"),
                    format!("fragment {index}").as_bytes(),
                )
            })
            .collect::<Vec<_>>();
        store
            .merge_staged("large-sync", &staged, UndoKind::Put)
            .unwrap();
    }
    assert_eq!(
        store.site_inventory("large-sync").unwrap().files.len(),
        FILES
    );

    let final_delta = (FILES - FINAL_DELTA..FILES)
        .map(|index| {
            stage_bytes(
                &format!("pagefind/fragment/{index:05}.pf_fragment"),
                format!("updated fragment {index}").as_bytes(),
            )
        })
        .collect::<Vec<_>>();
    store
        .merge_staged("large-sync", &final_delta, UndoKind::Put)
        .unwrap();
    store
        .put_file("large-sync", "pagefind/index.js", b"search index")
        .unwrap();
    assert_eq!(store.undo_stack("large-sync").unwrap().entries.len(), 10);
    assert!(matches!(
        store.lookup("large-sync", "pagefind/index.js"),
        Ok(Node::File { .. })
    ));
}

#[test]
fn unknown_stored_undo_kind_is_rejected_instead_of_mislabeled() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "index.html", b"content").unwrap();
    let token = store.undo_stack("hello").unwrap().entries[0].token.clone();
    let mut db = test_connection(&dir.path().join("symbol.db"));
    diesel::update(undo_operations::table.find(&token))
        .set(undo_operations::kind.eq(999_i64))
        .execute(&mut db)
        .unwrap();
    assert!(matches!(
        store.undo_stack("hello"),
        Err(StoreError::UnsupportedUndoKind(999))
    ));
}

#[test]
fn file_delete_and_site_create_have_isolated_undo_restoration() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("existing", "keep.txt", b"keep").unwrap();
    store.delete_file("existing", "keep.txt").unwrap();
    assert!(matches!(
        store.lookup("existing", "keep.txt"),
        Err(StoreError::NotFound)
    ));
    let delete = store.undo_stack("existing").unwrap().entries[0]
        .token
        .clone();
    store.undo("existing", Some(&delete)).unwrap();
    let hash = match store.lookup("existing", "keep.txt").unwrap() {
        Node::File { hash, .. } => hash,
        Node::Dir => panic!("expected restored file"),
    };
    assert_eq!(store.read_blob(hash).unwrap().as_ref(), b"keep");

    store.put_file("created", "index.html", b"new").unwrap();
    let create = store.undo_stack("created").unwrap().entries[0]
        .token
        .clone();
    store.undo("created", Some(&create)).unwrap();
    assert!(!store.site_exists("created"));
}

#[test]
#[expect(clippy::too_many_lines)]
fn expiry_inherits_refreshes_copies_moves_sweeps_and_undoes() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::<TestClock>::clone(&clock),
    )
    .unwrap();
    store.put_file("hello", "index.html", b"root").unwrap();
    store.put_file("hello", "assets/app.js", b"asset").unwrap();
    store
        .set_expiry(
            "hello",
            "",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 100,
            }),
        )
        .unwrap();
    store
        .set_expiry(
            "hello",
            "assets",
            Some(ExpiryPolicy::Decay(DecayPolicy {
                min_age_seconds: 50,
                max_age_seconds: 50,
                max_size_bytes: 1,
                power: 1.0,
            })),
        )
        .unwrap();
    store
        .set_expiry(
            "hello",
            "assets/app.js",
            Some(ExpiryPolicy::Absolute {
                deadline_unix_seconds: 1_700_000_075,
            }),
        )
        .unwrap();

    let report = store.expiry_report("hello", "assets/app.js").unwrap();
    assert_eq!(report.inherited_caps.len(), 2);
    assert_eq!(
        report.effective_expires_at.as_deref(),
        Some("2023-11-14T22:14:10Z")
    );
    assert_eq!(
        report.limited_by,
        Some(ExpiryLimit {
            kind: ExpiryTargetKind::Folder,
            path: Some("assets".to_string()),
        })
    );

    let folder_before = store
        .expiry_report("hello", "assets")
        .unwrap()
        .effective_expires_at;
    clock.advance(10_000);
    store.put_file("hello", "other.txt", b"other").unwrap();
    assert_eq!(
        store
            .expiry_report("hello", "assets")
            .unwrap()
            .effective_expires_at,
        folder_before
    );
    clock.advance(10_000);
    store
        .put_file("hello", "assets/app.js", b"changed")
        .unwrap();
    assert_ne!(
        store
            .expiry_report("hello", "assets")
            .unwrap()
            .effective_expires_at,
        folder_before
    );

    let disabled = store.set_expiry("hello", "assets/app.js", None).unwrap();
    assert!(disabled.report.own_policy.is_none());
    assert!(disabled.report.effective_expires_at.is_some());
    assert_eq!(
        disabled.report.limited_by.unwrap().kind,
        ExpiryTargetKind::Folder
    );

    clock.advance(10_000);
    let (copy, _) = store.copy_site("hello", Some("copy"), None).unwrap();
    let copied_site = store.expiry_report(&copy, "").unwrap();
    assert_eq!(
        copied_site.refreshed_at.as_deref(),
        Some("2023-11-14T22:13:50Z")
    );
    let copied_deadline = copied_site.effective_expires_at;
    store.move_site("copy", "moved").unwrap();
    assert_eq!(
        store
            .expiry_report("moved", "")
            .unwrap()
            .effective_expires_at,
        copied_deadline
    );
    let Node::File { hash, .. } = store.lookup("moved", MANIFEST_PATH).unwrap() else {
        panic!("manifest must exist");
    };
    let manifest = String::from_utf8(store.read_blob(hash).unwrap().to_vec()).unwrap();
    assert!(manifest.contains("[expiry.site]"));
    assert!(manifest.contains("[expiry.folders.\"assets\"]"));

    store.put_file("soon", "index.html", b"soon").unwrap();
    store
        .set_expiry(
            "soon",
            "index.html",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 1,
            }),
        )
        .unwrap();
    clock.advance(1_001);
    assert_eq!(store.sweep_expired().unwrap(), 1);
    assert!(!store.site_exists("soon"));
    let stack = store.undo_stack("soon").unwrap();
    assert_eq!(stack.entries[0].kind, "expire_sweep");
    store.undo("soon", Some(&stack.entries[0].token)).unwrap();
    assert!(store.site_exists("soon"));
    assert!(matches!(
        store.lookup("soon", "index.html"),
        Ok(Node::File { .. })
    ));
}

#[test]
fn partial_expiry_updates_aggregates_without_full_site_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::<TestClock>::clone(&clock),
    )
    .unwrap();
    store.put_file("hello", "assets/a.txt", b"aaa").unwrap();
    store.put_file("hello", "assets/b.txt", b"bb").unwrap();
    store.put_file("hello", "keep.txt", b"k").unwrap();
    store
        .set_expiry(
            "hello",
            "assets",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 1,
            }),
        )
        .unwrap();
    clock.advance(1_001);
    assert_eq!(store.sweep_expired().unwrap(), 1);
    assert!(matches!(
        store.lookup("hello", "keep.txt"),
        Ok(Node::File { .. })
    ));
    assert!(matches!(
        store.lookup("hello", "assets"),
        Err(StoreError::NotFound)
    ));
    let mut db = store.inner.readers.get();
    let site_id = site_id_locked(&mut db, "hello").unwrap();
    let aggregate = path_aggregates::table
        .find((site_id, ""))
        .select((path_aggregates::logical_bytes, path_aggregates::file_count))
        .first::<(i64, i64)>(&mut *db)
        .unwrap();
    assert_eq!(aggregate, (1, 1));
}

#[test]
fn copy_and_move_reuse_blobs_reject_conflicts_and_undo_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("source", "index.html", b"hello").unwrap();
    store.put_file("source", "assets/app.js", b"app").unwrap();
    let source = store.site_inventory("source").unwrap();

    let (_, copied) = store.copy_site("source", Some("copy"), None).unwrap();
    let copy = store.site_inventory("copy").unwrap();
    assert_eq!(copy.tree_hash, source.tree_hash);
    assert_eq!(copy.content_revision, source.content_revision);
    assert_eq!(
        copy.files
            .iter()
            .map(|file| (&file.path, &file.hash))
            .collect::<Vec<_>>(),
        source
            .files
            .iter()
            .map(|file| (&file.path, &file.hash))
            .collect::<Vec<_>>()
    );
    assert!(matches!(
        store.copy_site("source", Some("copy"), None),
        Err(StoreError::DestinationConflict)
    ));

    let (_, moved) = store.move_site("copy", "renamed").unwrap();
    assert!(!store.site_exists("copy"));
    assert_eq!(
        store.site_inventory("renamed").unwrap().tree_hash,
        source.tree_hash
    );
    let Node::File { hash, .. } = store.lookup("renamed", MANIFEST_PATH).unwrap() else {
        panic!("manifest must be a file");
    };
    let manifest = String::from_utf8(store.read_blob(hash).unwrap().to_vec()).unwrap();
    assert!(manifest.contains("name = \"renamed\""));

    store
        .undo("renamed", Some(&moved.undo.unwrap().token))
        .unwrap();
    assert!(store.site_exists("copy"));
    assert!(!store.site_exists("renamed"));
    store
        .undo("copy", Some(&copied.undo.unwrap().token))
        .unwrap();
    assert!(!store.site_exists("copy"));
    assert!(store.site_exists("source"));
}

#[test]
fn idempotency_replays_generated_resources_and_rejects_fingerprint_changes() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::<TestClock>::clone(&clock),
    )
    .unwrap();
    store.put_file("source", "index.html", b"source").unwrap();
    store.put_file("other", "index.html", b"other").unwrap();
    let idempotency = Idempotency {
        key: "retry-key".to_string(),
    };

    let (first, first_result) = store.copy_site("source", None, Some(&idempotency)).unwrap();
    let (replayed, replayed_result) = store.copy_site("source", None, Some(&idempotency)).unwrap();
    assert_eq!(replayed, first);
    assert_eq!(
        replayed_result.undo.unwrap().token,
        first_result.undo.unwrap().token
    );
    assert!(matches!(
        store.copy_site("other", None, Some(&idempotency)),
        Err(StoreError::IdempotencyConflict)
    ));

    clock.advance(u64::try_from(IDEMPOTENCY_RETENTION_MILLIS).unwrap() + 1);
    let (after_expiry, _) = store.copy_site("other", None, Some(&idempotency)).unwrap();
    assert_ne!(after_expiry, first);
}

#[test]
fn unnamed_put_idempotency_replays_without_creating_a_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    let upload = dir.path().join("upload");
    fs::write(&upload, b"first").unwrap();
    let idempotency = Idempotency {
        key: "unnamed-put".to_string(),
    };

    let (first, first_result) = store
        .publish_uploaded_file(
            None,
            "index.html",
            upload.clone(),
            PublishOptions {
                idempotency: Some(&idempotency),
                ..PublishOptions::default()
            },
        )
        .unwrap();
    let (replayed, replayed_result) = store
        .publish_uploaded_file(
            None,
            "index.html",
            upload.clone(),
            PublishOptions {
                idempotency: Some(&idempotency),
                ..PublishOptions::default()
            },
        )
        .unwrap();
    assert_eq!(replayed, first);
    assert_eq!(
        replayed_result.undo.unwrap().token,
        first_result.undo.unwrap().token
    );
    assert_eq!(store.stats().unwrap().sites, 1);

    fs::write(&upload, b"different").unwrap();
    assert!(matches!(
        store.publish_uploaded_file(
            None,
            "index.html",
            upload,
            PublishOptions {
                idempotency: Some(&idempotency),
                ..PublishOptions::default()
            }
        ),
        Err(StoreError::IdempotencyConflict)
    ));
    assert_eq!(store.stats().unwrap().sites, 1);
}

#[test]
fn replace_put_prunes_missing_paths_and_one_undo_restores_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "a.txt", b"a").unwrap();
    store.put_file("hello", "b.txt", b"b").unwrap();
    let upload = dir.path().join("a.txt");
    fs::write(&upload, b"a2").unwrap();
    let result = store
        .publish_uploaded_file(
            Some("hello"),
            "a.txt",
            upload,
            PublishOptions {
                replace: true,
                ..PublishOptions::default()
            },
        )
        .unwrap()
        .1;
    assert!(result.changed);
    assert!(matches!(
        store.lookup("hello", "b.txt"),
        Err(StoreError::NotFound)
    ));
    assert!(store.lookup("hello", "a.txt").is_ok());
    assert!(store.lookup("hello", "symbol.toml").is_ok());
    store
        .undo("hello", Some(&result.undo.unwrap().token))
        .unwrap();
    assert!(store.lookup("hello", "b.txt").is_ok());
    assert!(store.lookup("hello", "a.txt").is_ok());
}

#[test]
fn inventory_and_conditional_put_abort_strictly_on_drift() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "index.html", b"first").unwrap();
    let baseline = store.site_inventory("hello").unwrap();
    assert_eq!(baseline.files.len(), 1);
    assert_eq!(baseline.files[0].path, "index.html");
    assert_eq!(
        baseline.files[0].hash,
        format!("blake3:{}", blake3::hash(b"first").to_hex())
    );

    let update = dir.path().join("update");
    fs::write(&update, b"second").unwrap();
    let changed = store
        .put_uploaded_file("hello", "index.html", update, Some(&baseline.tree_hash))
        .unwrap();
    assert_eq!(changed.revision, baseline.content_revision + 1);

    let rejected = dir.path().join("rejected");
    fs::write(&rejected, b"must not publish").unwrap();
    let error = store
        .put_uploaded_file("hello", "new.txt", rejected, Some(&baseline.tree_hash))
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::PreconditionFailed { revision, .. } if revision == changed.revision
    ));
    assert!(matches!(
        store.lookup("hello", "new.txt"),
        Err(StoreError::NotFound)
    ));
}

/// The tree hash is a content hash, so expiry has to stay outside it.
///
/// `regenerate_site` finalises the hash over files, allocated entries and
/// aliases before it appends the expiry section to `symbol.toml`. That is
/// deliberate: it keeps the site `ETag` covering exactly what
/// `GET /{name}/FILES` inventory JSON reports, which excludes the generated
/// manifest for the same reason. Folding expiry in would change every existing
/// site's tree hash and invalidate stored `If-Match` values for a property that
/// is not content.
#[test]
fn expiry_changes_leave_the_content_tree_hash_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "index.html", b"body").unwrap();
    let baseline = store.site_inventory("hello").unwrap();

    store
        .set_expiry(
            "hello",
            "",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 604_800,
            }),
        )
        .unwrap();

    let after = store.site_inventory("hello").unwrap();
    assert_eq!(after.tree_hash, baseline.tree_hash);
    assert_eq!(after.content_revision, baseline.content_revision);

    // The manifest really did change, so this is an exclusion rather than a
    // no-op that happens to leave the hash alone.
    let Node::File { hash, .. } = store.lookup("hello", "symbol.toml").unwrap() else {
        panic!("generated manifest is a file");
    };
    let manifest = String::from_utf8(store.read_blob(hash).unwrap().to_vec()).unwrap();
    assert!(manifest.contains("[expiry.site]"), "{manifest}");
    assert!(manifest.contains("duration_seconds = 604800"), "{manifest}");

    // An `If-Match` carrying the pre-expiry hash must still be accepted.
    let update = dir.path().join("update");
    fs::write(&update, b"changed").unwrap();
    store
        .put_uploaded_file("hello", "index.html", update, Some(&baseline.tree_hash))
        .unwrap();

    // Content changes still move it.
    assert_ne!(
        store.site_inventory("hello").unwrap().tree_hash,
        baseline.tree_hash
    );
}

#[test]
fn sqlite_index_and_blob() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .replace_site("hello", b"<h1>x</h1>", Kind::Html, None, false)
        .unwrap();
    assert_eq!(
        store
            .list_sites()
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>(),
        vec!["hello".to_string()]
    );
    assert_eq!(
        store.list_files("hello").unwrap(),
        vec!["index.html".to_string(), "symbol.toml".to_string()]
    );
    assert!(dir.path().join("symbol.db").is_file());
    let Node::File { hash, .. } = store.lookup("hello", "index.html").unwrap() else {
        panic!("expected file");
    };
    let blob_path = store.blob_path(hash);
    assert_eq!(fs::read(&blob_path).unwrap(), b"<h1>x</h1>");
    let mut db = test_connection(&dir.path().join("symbol.db"));
    let stored_bytes = blobs::table
        .find(hash)
        .select(blobs::bytes)
        .first::<Vec<u8>>(&mut db)
        .unwrap();
    assert!(stored_bytes.is_empty());
    drop(db);
    assert_eq!(store.read_blob(hash).unwrap().as_ref(), b"<h1>x</h1>");
    let tar = store.pack_site("hello", ArchiveFormat::Tar).unwrap();
    assert_eq!(&tar[257..262], b"ustar");
    let tar_gz = store.pack_site("hello", ArchiveFormat::TarGz).unwrap();
    assert_eq!(&tar_gz[..2], [0x1f, 0x8b]);
    let zip = store.pack_site("hello", ArchiveFormat::Zip).unwrap();
    assert_eq!(&zip[..4], b"PK\x03\x04");
    assert_eq!(store.list_sites().unwrap().entries.len(), 1);
    let packed = store.pop_site("hello").unwrap();
    assert_eq!(&packed[..2], [0x1f, 0x8b]);
    assert!(store.list_sites().unwrap().entries.is_empty());
    assert_eq!(store.read_blob(hash).unwrap().as_ref(), b"<h1>x</h1>");
    assert!(blob_path.exists());
}

#[test]
fn startup_migrates_sqlite_blob_payloads_to_files() {
    let dir = tempfile::tempdir().unwrap();
    let content_hash = ContentHash::from(blake3::hash(b"legacy"));
    let hash_hex = content_hash.to_hex();
    {
        let mut db = test_connection(&dir.path().join("symbol.db"));
        run_migrations(&mut db).unwrap();
        diesel::insert_into(blobs::table)
            .values((
                blobs::hash.eq(content_hash),
                blobs::bytes.eq(b"legacy".as_slice()),
                blobs::size.eq(6_i64),
            ))
            .execute(&mut db)
            .unwrap();
        let site_id = diesel::insert_into(sites::table)
            .values(NewSite {
                name: "hello",
                created: Some(0),
                updated: 0,
                public_url: "",
                content_revision: 0,
                tree_hash: TreeHash::EMPTY,
                creator_kind: None,
                creator_hash: None,
                claim_hash: None,
                management_hash: None,
                management_status: 0,
            })
            .returning(sites::id)
            .get_result::<i64>(&mut db)
            .unwrap();
        ensure_file_entry(&mut db, site_id, "legacy.bin").unwrap();
        diesel::insert_into(files::table)
            .values(NewFile {
                site_id,
                path: "legacy.bin".to_string(),
                hash: content_hash,
                size: 6,
            })
            .execute(&mut db)
            .unwrap();
    }
    let target = dir
        .path()
        .join("blobs")
        .join(&hash_hex[..2])
        .join(&hash_hex[2..]);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"broken").unwrap();

    let store = Store::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(fs::read(store.blob_path(content_hash)).unwrap(), b"legacy");
    assert_eq!(store.read_blob(content_hash).unwrap(), "legacy");
    let mut db = test_connection(&dir.path().join("symbol.db"));
    assert_eq!(
        blobs::table
            .find(content_hash)
            .select(blobs::bytes)
            .first::<Vec<u8>>(&mut db)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        metadata::table
            .find("external_blobs_v1")
            .select(metadata::value)
            .first::<String>(&mut db)
            .unwrap(),
        "1"
    );
}

#[test]
fn startup_never_deletes_unreferenced_blob_files() {
    let dir = tempfile::tempdir().unwrap();
    drop(Store::new(dir.path().to_path_buf()).unwrap());
    let orphan_hash = "aa00000000000000000000000000000000000000000000000000000000000000";
    let orphan = dir
        .path()
        .join("blobs")
        .join(&orphan_hash[..2])
        .join(&orphan_hash[2..]);
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, b"orphan").unwrap();

    drop(Store::new(dir.path().to_path_buf()).unwrap());
    assert_eq!(fs::read(orphan).unwrap(), b"orphan");
}

#[test]
fn startup_restores_catalog_references_from_blob_quarantine() {
    let dir = tempfile::tempdir().unwrap();
    let hash;
    {
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store.put_file("hello", "live.bin", b"live").unwrap();
        let Node::File {
            hash: stored_hash, ..
        } = store.lookup("hello", "live.bin").unwrap()
        else {
            panic!("expected file");
        };
        hash = stored_hash;
    }
    let hash_hex = hash.to_hex();
    let live = dir
        .path()
        .join("blobs")
        .join(&hash_hex[..2])
        .join(&hash_hex[2..]);
    let quarantined = dir
        .path()
        .join("blobs")
        .join(".quarantine")
        .join(&hash_hex[..2])
        .join(&hash_hex[2..]);
    fs::create_dir_all(quarantined.parent().unwrap()).unwrap();
    fs::rename(&live, &quarantined).unwrap();

    let store = Store::new(dir.path().to_path_buf()).unwrap();

    assert_eq!(store.read_blob(hash).unwrap().as_ref(), b"live");
    assert!(live.exists());
    assert!(!quarantined.exists());
}

#[test]
fn put_file_rejects_junk() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .replace_site("hello", b"<h1>x</h1>", Kind::Html, None, false)
        .unwrap();
    let err = store
        .put_file("hello", "._index.html", &[0x00, 0x05, 0x16, 0x07])
        .unwrap_err();
    assert!(matches!(err, StoreError::Upload(UploadError::Junk)));
    assert_eq!(
        store.list_files("hello").unwrap(),
        vec!["index.html".to_string(), "symbol.toml".to_string()]
    );
}

#[test]
fn startup_does_not_delete_preexisting_junk_content() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .replace_site("hello", b"<h1>x</h1>", Kind::Html, None, false)
            .unwrap();
    }
    {
        let mut db = test_connection(&dir.path().join("symbol.db"));
        let apple = [0x00u8, 0x05, 0x16, 0x07, 0, 2, 0, 0];
        let apple_len = i64::try_from(apple.len()).unwrap();
        let hash = ContentHash::from(blake3::hash(&apple));
        let hash_hex = hash.to_hex();
        let blob = dir
            .path()
            .join("blobs")
            .join(&hash_hex[..2])
            .join(&hash_hex[2..]);
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::write(blob, apple).unwrap();
        diesel::insert_into(blobs::table)
            .values((
                blobs::hash.eq(hash),
                blobs::bytes.eq(Vec::<u8>::new()),
                blobs::size.eq(apple_len),
            ))
            .execute(&mut db)
            .unwrap();
        let site_id = site_id_locked(&mut db, "hello").unwrap();
        for path in ["._index.html", "keep.bin"] {
            ensure_file_entry(&mut db, site_id, path).unwrap();
            diesel::insert_into(files::table)
                .values(NewFile {
                    site_id,
                    path: path.to_string(),
                    hash,
                    size: apple_len,
                })
                .execute(&mut db)
                .unwrap();
        }
    }
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(
        store.list_files("hello").unwrap(),
        vec![
            "._index.html".to_string(),
            "index.html".to_string(),
            "keep.bin".to_string(),
            "symbol.toml".to_string(),
        ]
    );
}

#[test]
fn distributions_cover_empty_single_even_and_odd_populations() {
    let empty = distribution(&[]);
    assert!(empty.min.is_none());
    assert!(empty.mean.is_none());
    assert!(empty.stddev.is_none());

    let single = distribution(&[7]);
    assert_eq!(single.min, Some(7));
    assert_eq!(single.median, Some(7.0));
    assert_eq!(single.stddev, Some(0.0));

    let even = distribution(&[1, 2]);
    assert_eq!(even.p25, Some(1.25));
    assert_eq!(even.median, Some(1.5));
    assert_eq!(even.p75, Some(1.75));
    assert_eq!(even.stddev, Some(0.5));

    let odd = distribution(&[1, 2, 3]);
    assert_eq!(odd.p25, Some(1.5));
    assert_eq!(odd.median, Some(2.0));
    assert_eq!(odd.p75, Some(2.5));
    assert!((odd.stddev.unwrap() - (2.0_f64 / 3.0).sqrt()).abs() < 1e-12);
}

#[test]
fn stats_report_cross_site_deduplication() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("one", "a.txt", b"same").unwrap();
    store.put_file("two", "b.txt", b"same").unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.sites, 2);
    assert_eq!(stats.files, 2);
    assert_eq!(stats.blobs, 1);
    assert_eq!(stats.bytes, 4);
    assert_eq!(stats.logical_bytes, 8);
    assert_eq!(stats.saved_bytes, 4);
    assert!((stats.saved_fraction - 0.5).abs() < f64::EPSILON);
    assert_eq!(stats.file_sizes.median, Some(4.0));
    assert_eq!(stats.blob_sizes.median, Some(4.0));
}

#[test]
fn listings_include_recursive_file_counts_and_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "a/x.bin", &[1; 10]).unwrap();
    store.put_file("hello", "a/y.bin", &[2; 20]).unwrap();
    store.put_file("hello", "b/z.bin", &[3; 7]).unwrap();
    store.put_file("hello", "root.bin", &[4; 5]).unwrap();

    let root = store.list_dir("hello", "").unwrap();
    assert_eq!(root.files, 5);
    assert!(root.bytes > 42);
    assert_eq!(root.entries.len(), 4);
    assert_eq!(root.entries[0].kind, EntryKind::Directory);
    assert_eq!(root.entries[0].name, "a");
    assert_eq!(root.entries[0].files, 2);
    assert_eq!(root.entries[0].bytes, 30);
    assert_eq!(root.entries[1].name, "b");
    assert_eq!(root.entries[1].files, 1);
    assert_eq!(root.entries[1].bytes, 7);
    assert_eq!(root.entries[2].kind, EntryKind::File);
    assert_eq!(root.entries[2].name, "root.bin");
    assert_eq!(root.entries[2].bytes, 5);

    let nested = store.list_dir("hello", "a").unwrap();
    assert_eq!(nested.files, 2);
    assert_eq!(nested.bytes, 30);

    let sites = store.list_sites().unwrap();
    assert_eq!(sites.files, 4);
    assert_eq!(sites.bytes, 42);
    assert_eq!(sites.entries[0].files, 4);
    assert_eq!(sites.entries[0].bytes, 42);
}

#[test]
fn lookup_distinguishes_files_directories_and_missing_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "docs/index.html", b"docs").unwrap();

    assert!(matches!(store.lookup("hello", ""), Ok(Node::Dir)));
    assert!(matches!(store.lookup("hello", "docs"), Ok(Node::Dir)));
    let Node::File { logical, hash } = store.lookup("hello", "docs/index.html").unwrap() else {
        panic!("expected file");
    };
    assert_eq!(logical, "docs/index.html");
    assert_eq!(hash, ContentHash::from(blake3::hash(b"docs")));
    assert!(matches!(
        store.lookup("hello", "missing"),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.lookup("absent", ""),
        Err(StoreError::NotFound)
    ));
}

#[test]
fn nested_listing_and_delete_use_literal_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "a%/one.txt", b"one").unwrap();
    store.put_file("hello", "a_/two.txt", b"two").unwrap();
    store.put_file("hello", "a0/three.txt", b"three").unwrap();

    let listing = store.list_dir("hello", "a%").unwrap();
    assert_eq!(listing.files, 1);
    assert_eq!(listing.entries[0].name, "one.txt");

    store.delete_file("hello", "a%").unwrap();
    assert!(matches!(
        store.lookup("hello", "a%/one.txt"),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.lookup("hello", "a_/two.txt"),
        Ok(Node::File { .. })
    ));
    assert!(matches!(
        store.lookup("hello", "a0/three.txt"),
        Ok(Node::File { .. })
    ));
}

/// A distinct, non-zero hash per label, for tests that only need keys.
fn cache_key(label: u8) -> ContentHash {
    ContentHash::from_bytes([label; 32])
}

#[test]
fn blob_cache_is_byte_bounded_and_evicts_least_recently_used() {
    // Two entries exactly: 3 payload bytes plus a key charged at twice its
    // size plus BLOB_CACHE_ENTRY_OVERHEAD.
    let capacity = 2 * (3 + size_of::<ContentHash>() * 2 + BLOB_CACHE_ENTRY_OVERHEAD);
    let cache = BlobCache::new(capacity, 16, Arc::new(Metrics::default()));
    cache.insert(cache_key(b'a'), Bytes::from_static(b"aaa"));
    cache.insert(cache_key(b'b'), Bytes::from_static(b"bbb"));
    assert_eq!(cache.get(cache_key(b'a')).unwrap(), "aaa");

    cache.insert(cache_key(b'c'), Bytes::from_static(b"ccc"));

    assert!(cache.contains(cache_key(b'a')));
    assert!(!cache.contains(cache_key(b'b')));
    assert!(cache.contains(cache_key(b'c')));
    let state = cache.state.lock().unwrap();
    assert!(state.charge <= cache.capacity);
    assert_eq!(state.recency.len(), state.entries.len());
}

#[test]
fn blob_cache_caps_entry_count() {
    let cache = BlobCache::new(usize::MAX, 2, Arc::new(Metrics::default()));
    cache.insert(cache_key(b'a'), Bytes::new());
    cache.insert(cache_key(b'b'), Bytes::new());
    cache.insert(cache_key(b'c'), Bytes::new());

    assert!(!cache.contains(cache_key(b'a')));
    assert!(cache.contains(cache_key(b'b')));
    assert!(cache.contains(cache_key(b'c')));
}

#[test]
fn serving_metrics_count_cache_activity_and_reader_waits() {
    let metrics = Arc::new(Metrics::default());
    let cache = BlobCache::new(1024, 1, Arc::clone(&metrics));
    assert!(cache.get(cache_key(b'z')).is_none());
    cache.insert(cache_key(b'a'), Bytes::from_static(b"a"));
    assert_eq!(cache.get(cache_key(b'a')).unwrap(), "a");
    cache.insert(cache_key(b'b'), Bytes::from_static(b"b"));

    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    let held: Vec<_> = (0..store.inner.readers.size())
        .map(|_| store.inner.readers.get())
        .collect();
    let concurrent = store.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        concurrent.list_sites().unwrap();
    });
    started_rx.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    drop(held);
    thread.join().unwrap();

    let cache_stats = metrics.snapshot().cache;
    assert_eq!(cache_stats.hits, 1);
    assert_eq!(cache_stats.misses, 1);
    assert_eq!(cache_stats.evictions, 1);
    let reader_stats = store.inner.metrics.snapshot().readers;
    assert_eq!(reader_stats.waits, 1);
    assert!(reader_stats.operations >= 1);
    assert!(reader_stats.wait_micros > 0);
}

#[test]
fn repeated_blob_reads_share_cached_storage_and_gc_evicts_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "index.html", b"first").unwrap();
    let Node::File { hash, .. } = store.lookup("hello", "index.html").unwrap() else {
        panic!("expected file");
    };

    let first = store.read_blob(hash).unwrap();
    let second = store.read_blob(hash).unwrap();
    assert_eq!(first.as_ptr(), second.as_ptr());
    assert!(store.inner.blobs.contains(hash));

    store.put_file("hello", "index.html", b"second").unwrap();
    assert!(store.inner.blobs.contains(hash));
    assert_eq!(store.read_blob(hash).unwrap(), "first");
}

#[test]
fn reader_pool_serves_another_query_while_one_reader_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "index.html", b"hello").unwrap();
    let held = store.inner.readers.get();
    let concurrent = store.clone();
    let (sent, received) = std::sync::mpsc::channel();

    let thread = std::thread::spawn(move || {
        sent.send(concurrent.list_sites().unwrap()).unwrap();
    });
    let sites = received
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("second reader should not wait for the first");

    assert_eq!(sites.entries[0].name, "hello");
    drop(held);
    thread.join().unwrap();
}

#[test]
fn concurrent_reads_and_disjoint_writes_remain_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("hello", "index.html", b"hello").unwrap();
    let start = Arc::new(std::sync::Barrier::new(9));

    std::thread::scope(|scope| {
        for writer in 0..4 {
            let store = store.clone();
            let start = Arc::clone(&start);
            scope.spawn(move || {
                start.wait();
                for file in 0..10 {
                    store
                        .put_file("hello", &format!("writer-{writer}/{file}.txt"), b"value")
                        .unwrap();
                }
            });
        }
        for _ in 0..4 {
            let store = store.clone();
            let start = Arc::clone(&start);
            scope.spawn(move || {
                start.wait();
                for _ in 0..50 {
                    assert!(matches!(
                        store.lookup("hello", "index.html"),
                        Ok(Node::File { .. })
                    ));
                    assert!(store.list_dir("hello", "").unwrap().files >= 1);
                }
            });
        }
        start.wait();
    });

    assert_eq!(store.list_files("hello").unwrap().len(), 42);
}

#[test]
fn concurrent_upload_paths_are_unique_within_one_clock_tick() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store =
        Store::with_clock(dir.path().to_path_buf(), "http://symbol".to_string(), clock).unwrap();
    let paths = Arc::new(Mutex::new(HashSet::new()));
    std::thread::scope(|scope| {
        for _ in 0..32 {
            let store = store.clone();
            let paths = Arc::clone(&paths);
            scope.spawn(move || {
                paths.lock().unwrap().insert(store.upload_path());
            });
        }
    });
    assert_eq!(paths.lock().unwrap().len(), 32);
}

fn node_hash(store: &Store, site: &str, path: &str) -> ContentHash {
    let Node::File { hash, .. } = store.lookup(site, path).unwrap() else {
        panic!("expected file");
    };
    hash
}

/// Parse a hash back out of a receipt, whose fields stay wire strings.
fn wire_hash(value: &str) -> ContentHash {
    ContentHash::parse_wire(value).expect("receipt carries a valid hash")
}

#[test]
fn single_file_mutation_on_42802_file_site_stores_one_delta() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("large", "index.html", b"old").unwrap();
    let hash = node_hash(&store, "large", "index.html");
    let hash_hex = hash.to_hex();
    {
        let mut db = store.inner.writer.lock().unwrap();
        let site_id = site_id_locked(&mut db, "large").unwrap();
        diesel::sql_query(format!(
            "WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x + 1 FROM n WHERE x < 42800)
             INSERT INTO site_entries(site_id, path, kind)
             SELECT {site_id}, printf('bulk/%05d', x), 0 FROM n"
        ))
        .execute(&mut *db)
        .unwrap();
        diesel::sql_query(format!(
            "INSERT INTO files(site_id, path, kind, hash, size)
             SELECT site_id, path, 0, unhex('{hash_hex}'), 3
             FROM site_entries WHERE site_id = {site_id} AND path LIKE 'bulk/%'"
        ))
        .execute(&mut *db)
        .unwrap();
    }
    store.put_file("large", "index.html", b"new").unwrap();
    let mut db = test_connection(&dir.path().join("symbol.db"));
    let token = undo_operations::table
        .inner_join(undo_names::table.on(undo_names::token.eq(undo_operations::token)))
        .filter(undo_names::name.eq("large"))
        .select(undo_operations::token)
        .order(undo_operations::rowid.desc())
        .first::<String>(&mut db)
        .unwrap();
    assert_eq!(
        undo_file_deltas::table
            .filter(undo_file_deltas::token.eq(&token))
            .select(count_star())
            .first::<i64>(&mut db)
            .unwrap(),
        1
    );
    assert_eq!(
        undo_files::table
            .filter(undo_files::token.eq(&token))
            .select(count_star())
            .first::<i64>(&mut db)
            .unwrap(),
        0
    );
}

#[test]
#[expect(clippy::cognitive_complexity, clippy::too_many_lines)]
fn allocated_names_are_exact_idempotent_and_relocate_with_undo() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("assets", "index.html", b"site").unwrap();
    let idempotency = Idempotency {
        key: "allocated-once".to_string(),
    };
    let options = FileMutationOptions {
        idempotency: Some(&idempotency),
        ..FileMutationOptions::default()
    };
    let first = store
        .allocate_bytes(
            "assets",
            b"payload",
            AllocationSpec {
                folder: "nested/assets",
                naming: AllocatedName {
                    prefix: "pre-",
                    suffix: "-final",
                    extension: Some("..JP G.."),
                },
                media_type: "IMAGE/JPEG",
            },
            options,
        )
        .unwrap();
    assert_eq!(
        first.path,
        format!(
            "nested/assets/pre-{}-final.jp-g",
            blake3::hash(b"payload").to_hex()
        )
    );
    assert_eq!(
        store.read_blob(wire_hash(&first.hash)).unwrap().as_ref(),
        b"payload"
    );
    let first_metadata = store.allocated_metadata("assets", &first.path).unwrap();
    assert_eq!(
        first_metadata.naming_mode,
        AllocatedNamingMode::ContentAddressed
    );
    assert_eq!(first_metadata.prefix, "pre-");
    assert_eq!(first_metadata.suffix, "-final");
    assert_eq!(first_metadata.extension.as_deref(), Some("jp-g"));
    assert_eq!(first_metadata.media_type, "image/jpeg");
    store
        .copy_site("assets", Some("assets-copy"), None)
        .unwrap();
    assert_eq!(
        store
            .allocated_metadata("assets-copy", &first.path)
            .unwrap(),
        first_metadata
    );
    let replay = store
        .allocate_bytes(
            "assets",
            b"payload",
            AllocationSpec {
                folder: "nested/assets",
                naming: AllocatedName {
                    prefix: "pre-",
                    suffix: "-final",
                    extension: Some("..JP G.."),
                },
                media_type: "image/jpeg",
            },
            options,
        )
        .unwrap();
    assert!(replay.replayed);
    let source = dir.path().join("from-file.bin");
    fs::write(&source, b"file payload").unwrap();
    let from_file = store
        .allocate_file(
            "assets",
            &source,
            AllocationSpec {
                folder: "",
                naming: AllocatedName {
                    prefix: "file-",
                    suffix: "",
                    extension: Some("bin"),
                },
                media_type: "application/octet-stream",
            },
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        store
            .read_blob(wire_hash(&from_file.hash))
            .unwrap()
            .as_ref(),
        b"file payload"
    );
    store
        .set_expiry(
            "assets",
            &first.path,
            Some(ExpiryPolicy::Relative {
                duration_seconds: 600,
            }),
        )
        .unwrap();
    let moved = store
        .replace_allocated(
            "assets",
            &first.path,
            AllocationSource::Bytes(b"changed"),
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_ne!(moved.path, first.path);
    let moved_metadata = store.allocated_metadata("assets", &moved.path).unwrap();
    assert_eq!(moved_metadata.prefix, first_metadata.prefix);
    assert_eq!(moved_metadata.suffix, first_metadata.suffix);
    assert_eq!(moved_metadata.extension, first_metadata.extension);
    assert_eq!(moved_metadata.media_type, first_metadata.media_type);
    assert!(matches!(
        store.lookup("assets", &first.path),
        Err(StoreError::NotFound)
    ));
    store.expiry_report("assets", &moved.path).unwrap();
    assert!(store.blob_path(wire_hash(&first.hash)).is_file());
    store.undo("assets", None).unwrap();
    assert_eq!(
        node_hash(&store, "assets", &first.path),
        wire_hash(&first.hash)
    );
    assert_eq!(
        store.allocated_metadata("assets", &first.path).unwrap(),
        first_metadata
    );
    store.expiry_report("assets", &first.path).unwrap();
    assert!(matches!(
        store.lookup("assets", &moved.path),
        Err(StoreError::NotFound)
    ));
    store.delete_file("assets", &first.path).unwrap();
    assert!(store.blob_path(wire_hash(&first.hash)).is_file());
    store.undo("assets", None).unwrap();
    assert_eq!(
        node_hash(&store, "assets", &first.path),
        wire_hash(&first.hash)
    );
}

#[test]
#[expect(clippy::too_many_lines)]
fn allocation_replacement_and_splice_redact_before_hashing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("sanitize-mutations", "regular.txt", b"before")
        .unwrap();
    store
        .put_file("sanitize-mutations", "splice.txt", b"prefix:")
        .unwrap();
    let management = format!("sym_mgmt_{}", "a".repeat(64));
    let claim = format!("sym_claim_{}", "b".repeat(64));
    let redacted_management = format!("sym_mgmt_{}", "*".repeat(64));
    let redacted_claim = format!("sym_claim_{}", "*".repeat(64));
    let allocation_input = format!("{management}\n{claim}");
    let allocation_output = format!("{redacted_management}\n{redacted_claim}");

    let allocated = store
        .allocate_bytes(
            "sanitize-mutations",
            allocation_input.as_bytes(),
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        allocated.mutation.as_ref().unwrap().sanitized,
        TokenCounts {
            management: 1,
            claim: 1
        }
    );
    assert_eq!(
        allocated.hash,
        format!(
            "blake3:{}",
            blake3::hash(allocation_output.as_bytes()).to_hex()
        )
    );
    let hash_hex = allocated
        .hash
        .strip_prefix("blake3:")
        .unwrap_or(&allocated.hash);
    assert!(allocated.path.contains(hash_hex));
    assert_eq!(
        store
            .read_blob(wire_hash(&allocated.hash))
            .unwrap()
            .as_ref(),
        allocation_output.as_bytes()
    );

    let proposal = store
        .propose_allocation(
            "sanitize-mutations",
            AllocationSource::Bytes(management.as_bytes()),
            PendingAllocationSpec::default(),
            None,
        )
        .unwrap();
    assert_eq!(
        proposal.hash,
        format!(
            "blake3:{}",
            blake3::hash(redacted_management.as_bytes()).to_hex()
        )
    );
    let finalized = store
        .finalize_allocation_custom(
            "sanitize-mutations",
            &proposal.token,
            "pending.txt",
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        finalized.mutation.as_ref().unwrap().sanitized,
        TokenCounts {
            management: 1,
            claim: 0
        }
    );

    let regular_hash = node_hash(&store, "sanitize-mutations", "regular.txt");
    let replaced = store
        .replace_file_content(
            "sanitize-mutations",
            "regular.txt",
            &regular_hash.to_hex(),
            AllocationSource::Bytes(claim.as_bytes()),
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        replaced.mutation.as_ref().unwrap().sanitized,
        TokenCounts {
            management: 0,
            claim: 1
        }
    );
    assert_eq!(
        store.read_blob(wire_hash(&replaced.hash)).unwrap().as_ref(),
        redacted_claim.as_bytes()
    );

    let splice_hash = node_hash(&store, "sanitize-mutations", "splice.txt");
    let spliced = store
        .splice_file(
            "sanitize-mutations",
            "splice.txt",
            &splice_hash.to_hex(),
            &[Splice {
                offset: 7,
                delete: 0,
                insert: SpliceSource::Bytes(management.as_bytes()),
            }],
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        spliced.mutation.as_ref().unwrap().sanitized,
        TokenCounts {
            management: 1,
            claim: 0
        }
    );
    assert_eq!(
        store.read_blob(wire_hash(&spliced.hash)).unwrap().as_ref(),
        format!("prefix:{redacted_management}").as_bytes()
    );
}

#[test]
fn allocated_extension_normalization_matches_shared_sdk_vectors() {
    for vector in symbol_contract::EXTENSION_NORMALIZATION_VECTORS {
        assert_eq!(
            normalize_allocated_extension(vector.input).ok().as_deref(),
            vector.output,
            "{:?}",
            vector.input
        );
    }
}

#[test]
#[expect(clippy::too_many_lines)]
fn pending_allocation_finalizes_once_and_pruning_releases_blob() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::clone(&clock) as Arc<dyn Clock>,
    )
    .unwrap();
    store.put_file("pending", "index.html", b"site").unwrap();
    let proposal = store
        .propose_allocation(
            "pending",
            AllocationSource::Bytes(b"one"),
            PendingAllocationSpec {
                folder: "nested/pending",
                media_type: "application/custom",
                extension: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(proposal.size, 3);
    assert_eq!(proposal.folder, "nested/pending");
    assert_eq!(proposal.media_type, "application/custom");
    assert!(proposal.expires_at.ends_with('Z'));
    store.put_file("other", "index.html", b"other").unwrap();
    assert!(matches!(
        store.finalize_allocation(
            "other",
            &proposal.token,
            AllocatedName::default(),
            FileMutationOptions::default()
        ),
        Err(StoreError::InvalidPendingAllocation)
    ));
    let proposal_hash = ContentHash::try_from(proposal.hash.as_str()).unwrap();
    let mut db = test_connection(&dir.path().join("symbol.db"));
    assert_eq!(
        blobs::table
            .filter(blobs::hash.eq(proposal_hash))
            .select(count_star())
            .first::<i64>(&mut db)
            .unwrap(),
        1
    );
    drop(db);
    let finalized = store
        .finalize_allocation(
            "pending",
            &proposal.token,
            AllocatedName {
                prefix: "custom-",
                suffix: "",
                extension: Some("BIN"),
            },
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(finalized.hash, proposal.hash);
    assert!(finalized.path.starts_with("nested/pending/custom-"));
    let mut db = test_connection(&dir.path().join("symbol.db"));
    assert_eq!(
        blobs::table
            .filter(blobs::hash.eq(proposal_hash))
            .select(count_star())
            .first::<i64>(&mut db)
            .unwrap(),
        1
    );
    drop(db);
    assert!(matches!(
        store.finalize_allocation(
            "pending",
            &proposal.token,
            AllocatedName::default(),
            FileMutationOptions::default()
        ),
        Err(StoreError::InvalidPendingAllocation)
    ));

    let abandoned = store
        .propose_allocation(
            "pending",
            AllocationSource::Bytes(b"abandoned"),
            PendingAllocationSpec::default(),
            None,
        )
        .unwrap();
    assert!(store.blob_path(wire_hash(&abandoned.hash)).is_file());
    clock.advance(u64::try_from(PENDING_RETENTION_MILLIS).unwrap() + 1);
    assert_eq!(store.prune_pending_allocations().unwrap(), 1);
    assert!(!store.blob_path(wire_hash(&abandoned.hash)).exists());
    assert!(blob_quarantine_path(dir.path(), &abandoned.hash).is_file());

    let cancelled = store
        .propose_allocation(
            "pending",
            AllocationSource::Bytes(b"cancelled"),
            PendingAllocationSpec::default(),
            None,
        )
        .unwrap();
    store
        .cancel_allocation("pending", &cancelled.token, None)
        .unwrap();
    assert!(!store.blob_path(wire_hash(&cancelled.hash)).exists());
    assert!(blob_quarantine_path(dir.path(), &cancelled.hash).is_file());
}

#[test]
fn custom_finalize_validates_basename_and_reports_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("custom", "index.html", b"site").unwrap();
    let proposal = store
        .propose_allocation(
            "custom",
            AllocationSource::Bytes(b"custom payload"),
            PendingAllocationSpec {
                folder: "nested/custom",
                media_type: "text/plain",
                extension: None,
            },
            None,
        )
        .unwrap();
    for invalid in ["", ".", "..", "../escape", "a/b", r"a\b", "UNDO"] {
        assert!(matches!(
            store.finalize_allocation_custom(
                "custom",
                &proposal.token,
                invalid,
                FileMutationOptions::default()
            ),
            Err(StoreError::InvalidAllocatedName)
        ));
    }
    let finalized = store
        .finalize_allocation_custom(
            "custom",
            &proposal.token,
            "safe-name.txt",
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(finalized.path, "nested/custom/safe-name.txt");
    let metadata = store.allocated_metadata("custom", &finalized.path).unwrap();
    assert_eq!(metadata.naming_mode, AllocatedNamingMode::Custom);
    assert_eq!(metadata.media_type, "text/plain");

    store
        .put_file("custom", "nested/custom/taken.txt", b"taken")
        .unwrap();
    let conflict = store
        .propose_allocation(
            "custom",
            AllocationSource::Bytes(b"conflict"),
            PendingAllocationSpec {
                folder: "nested/custom",
                media_type: "text/plain",
                extension: None,
            },
            None,
        )
        .unwrap();
    assert!(matches!(
        store.finalize_allocation_custom(
            "custom",
            &conflict.token,
            "taken.txt",
            FileMutationOptions::default()
        ),
        Err(StoreError::DestinationConflict)
    ));
    store
        .cancel_allocation("custom", &conflict.token, None)
        .unwrap();
}

#[test]
fn allocated_expiry_sweep_and_whole_site_undo_keep_blob_reachable() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::clone(&clock) as Arc<dyn Clock>,
    )
    .unwrap();
    store
        .put_file("expiry-allocated", "index.html", b"site")
        .unwrap();
    let allocated = store
        .allocate_bytes(
            "expiry-allocated",
            b"expires",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "expiry-allocated",
            &allocated.path,
            Some(ExpiryPolicy::Relative {
                duration_seconds: 1,
            }),
        )
        .unwrap();
    clock.advance(1_001);
    assert_eq!(store.sweep_expired().unwrap(), 1);
    assert!(matches!(
        store.lookup("expiry-allocated", &allocated.path),
        Err(StoreError::NotFound)
    ));
    assert!(store.blob_path(wire_hash(&allocated.hash)).is_file());
    store.undo("expiry-allocated", None).unwrap();
    assert_eq!(
        node_hash(&store, "expiry-allocated", &allocated.path),
        wire_hash(&allocated.hash)
    );
}

#[test]
fn replacement_requires_current_hash_and_preserves_regular_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("replace", "data.bin", b"old").unwrap();
    let old_hash = node_hash(&store, "replace", "data.bin");
    assert!(matches!(
        store.replace_file_content(
            "replace",
            "data.bin",
            "stale",
            AllocationSource::Bytes(b"new"),
            FileMutationOptions::default()
        ),
        Err(StoreError::StaleContentHash(current)) if current == old_hash
    ));
    let replaced = store
        .replace_file_content(
            "replace",
            "data.bin",
            &old_hash.to_hex(),
            AllocationSource::Bytes(b"new"),
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(replaced.path, "data.bin");
    store.undo("replace", None).unwrap();
    assert_eq!(node_hash(&store, "replace", "data.bin"), old_hash);
}

#[test]
fn multi_splice_uses_original_offsets_and_validates_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("splice", "data.txt", b"0123456789").unwrap();
    let base = node_hash(&store, "splice", "data.txt");
    let result = store
        .splice_file(
            "splice",
            "data.txt",
            &base.to_hex(),
            &[
                Splice {
                    offset: 0,
                    delete: 0,
                    insert: SpliceSource::Bytes(b"A"),
                },
                Splice {
                    offset: 2,
                    delete: 3,
                    insert: SpliceSource::Bytes(b"BC"),
                },
                Splice {
                    offset: 10,
                    delete: 0,
                    insert: SpliceSource::Bytes(b"Z"),
                },
            ],
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        store.read_blob(wire_hash(&result.hash)).unwrap().as_ref(),
        b"A01BC56789Z"
    );
    assert!(matches!(
        store.splice_file(
            "splice",
            "data.txt",
            &result.hash,
            &[
                Splice {
                    offset: 4,
                    delete: 2,
                    insert: SpliceSource::Empty,
                },
                Splice {
                    offset: 5,
                    delete: 0,
                    insert: SpliceSource::Empty,
                }
            ],
            FileMutationOptions::default()
        ),
        Err(StoreError::InvalidSpliceOrder)
    ));
    assert!(matches!(
        store.splice_file(
            "splice",
            "data.txt",
            &result.hash,
            &[Splice {
                offset: 99,
                delete: 0,
                insert: SpliceSource::Empty,
            }],
            FileMutationOptions::default()
        ),
        Err(StoreError::SpliceRange)
    ));
    let secret = format!("sym_mgmt_{}\n", "a".repeat(64));
    let sanitized = store
        .splice_file(
            "splice",
            "data.txt",
            &result.hash,
            &[Splice {
                offset: 0,
                delete: 0,
                insert: SpliceSource::Bytes(secret.as_bytes()),
            }],
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(sanitized.mutation.unwrap().sanitized.management, 1);
    assert_eq!(
        store
            .read_blob(wire_hash(&sanitized.hash))
            .unwrap()
            .as_ref(),
        format!("sym_mgmt_{}\nA01BC56789Z", "*".repeat(64)).as_bytes()
    );
}

#[test]
fn splice_streams_large_file_and_file_insertions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    let source = dir.path().join("large.bin");
    let insertion = dir.path().join("insert.bin");
    {
        let mut file = fs::File::create(&source).unwrap();
        for _ in 0..256 {
            file.write_all(&[7_u8; 64 * 1024]).unwrap();
        }
    }
    fs::write(&insertion, [8_u8; 64 * 1024]).unwrap();
    store
        .put_uploaded_file("stream", "large.bin", source, None)
        .unwrap();
    let base = node_hash(&store, "stream", "large.bin");
    let result = store
        .splice_file(
            "stream",
            "large.bin",
            &base.to_hex(),
            &[Splice {
                offset: 8 * 1024 * 1024,
                delete: 64 * 1024,
                insert: SpliceSource::File(&insertion),
            }],
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(result.size, 16 * 1024 * 1024);
    let mut file = fs::File::open(store.blob_path(wire_hash(&result.hash))).unwrap();
    file.seek(SeekFrom::Start(8 * 1024 * 1024)).unwrap();
    let mut marker = [0_u8; 1];
    file.read_exact(&mut marker).unwrap();
    assert_eq!(marker, [8]);
}

#[test]
fn writer_transaction_rechecks_replace_and_splice_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("cas", "data.txt", b"base").unwrap();
    let base = node_hash(&store, "cas", "data.txt");
    let racer = store.clone();
    store.set_before_content_commit(move || {
        racer.put_file("cas", "data.txt", b"racer").unwrap();
    });
    assert!(matches!(
        store.replace_file_content(
            "cas",
            "data.txt",
            &base.to_hex(),
            AllocationSource::Bytes(b"replacement"),
            FileMutationOptions::default(),
        ),
        Err(StoreError::StaleContentHash(current))
            if current == ContentHash::from(blake3::hash(b"racer"))
    ));
    assert_eq!(
        store
            .read_blob(node_hash(&store, "cas", "data.txt"))
            .unwrap()
            .as_ref(),
        b"racer"
    );

    let splice_base = node_hash(&store, "cas", "data.txt");
    let racer = store.clone();
    store.set_before_content_commit(move || {
        racer.put_file("cas", "data.txt", b"second racer").unwrap();
    });
    assert!(matches!(
        store.splice_file(
            "cas",
            "data.txt",
            &splice_base.to_hex(),
            &[Splice {
                offset: 0,
                delete: 0,
                insert: SpliceSource::Bytes(b"x"),
            }],
            FileMutationOptions::default(),
        ),
        Err(StoreError::StaleContentHash(current))
            if current == ContentHash::from(blake3::hash(b"second racer"))
    ));

    let pending = store
        .propose_allocation(
            "cas",
            AllocationSource::Bytes(b"allocated base"),
            PendingAllocationSpec::default(),
            None,
        )
        .unwrap();
    let allocated = store
        .finalize_allocation_custom(
            "cas",
            &pending.token,
            "fixed.bin",
            FileMutationOptions::default(),
        )
        .unwrap();
    let racer = store.clone();
    let allocated_path = allocated.path.clone();
    store.set_before_content_commit(move || {
        racer
            .replace_allocated(
                "cas",
                &allocated_path,
                AllocationSource::Bytes(b"allocated racer"),
                FileMutationOptions::default(),
            )
            .unwrap();
    });
    assert!(matches!(
        store.replace_file_content(
            "cas",
            &allocated.path,
            &allocated.hash,
            AllocationSource::Bytes(b"allocated replacement"),
            FileMutationOptions::default(),
        ),
        Err(StoreError::StaleContentHash(current))
            if current == ContentHash::from(blake3::hash(b"allocated racer"))
    ));
    assert!(
        !store
            .blob_path(ContentHash::from(blake3::hash(b"allocated replacement")))
            .exists()
    );
}

#[test]
fn file_sources_are_staged_once_before_later_reads() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("staged", "data.txt", b"base").unwrap();
    let allocation_source = dir.path().join("allocation-source");
    fs::write(&allocation_source, b"original allocation").unwrap();
    let changed_source = allocation_source.clone();
    store.set_before_content_commit(move || {
        fs::write(changed_source, b"changed allocation").unwrap();
    });
    let allocated = store
        .allocate_file(
            "staged",
            &allocation_source,
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(allocated.size, b"original allocation".len() as u64);
    assert_eq!(
        store
            .read_blob(wire_hash(&allocated.hash))
            .unwrap()
            .as_ref(),
        b"original allocation"
    );

    let insertion = dir.path().join("splice-source");
    fs::write(&insertion, b"original insertion").unwrap();
    let changed_insertion = insertion.clone();
    store.set_before_content_commit(move || {
        fs::write(changed_insertion, b"changed insertion").unwrap();
    });
    let base = node_hash(&store, "staged", "data.txt");
    let spliced = store
        .splice_file(
            "staged",
            "data.txt",
            &base.to_hex(),
            &[Splice {
                offset: 4,
                delete: 0,
                insert: SpliceSource::File(&insertion),
            }],
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        store.read_blob(wire_hash(&spliced.hash)).unwrap().as_ref(),
        b"baseoriginal insertion"
    );
}

#[test]
fn allocated_replacement_reuses_existing_destination_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("reuse", "index.html", b"site").unwrap();
    let first = store
        .allocate_bytes(
            "reuse",
            b"first",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    let destination = store
        .allocate_bytes(
            "reuse",
            b"destination",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();

    let relocated = store
        .replace_allocated(
            "reuse",
            &first.path,
            AllocationSource::Bytes(b"destination"),
            FileMutationOptions::default(),
        )
        .unwrap();

    assert!(relocated.changed);
    assert_eq!(relocated.path, destination.path);
    assert!(matches!(
        store.lookup("reuse", &first.path),
        Err(StoreError::NotFound)
    ));
    assert_eq!(
        node_hash(&store, "reuse", &destination.path),
        wire_hash(&destination.hash)
    );
    store.undo("reuse", None).unwrap();
    assert_eq!(
        node_hash(&store, "reuse", &first.path),
        wire_hash(&first.hash)
    );
    assert_eq!(
        node_hash(&store, "reuse", &destination.path),
        wire_hash(&destination.hash)
    );
}

#[test]
fn pending_finalize_and_regular_noop_replay_after_later_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("replay", "data.txt", b"same").unwrap();
    let base = node_hash(&store, "replay", "data.txt");
    let noop_key = Idempotency {
        key: "regular-noop".to_string(),
    };
    let noop_options = FileMutationOptions {
        idempotency: Some(&noop_key),
        ..FileMutationOptions::default()
    };
    let noop = store
        .replace_file_content(
            "replay",
            "data.txt",
            &base.to_hex(),
            AllocationSource::Bytes(b"same"),
            noop_options,
        )
        .unwrap();
    assert!(!noop.changed);
    store.put_file("replay", "data.txt", b"later").unwrap();
    let replay = store
        .replace_file_content(
            "replay",
            "data.txt",
            &base.to_hex(),
            AllocationSource::Bytes(b"same"),
            noop_options,
        )
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(
        node_hash(&store, "replay", "data.txt"),
        ContentHash::from(blake3::hash(b"later"))
    );

    let pending = store
        .propose_allocation(
            "replay",
            AllocationSource::Bytes(b"pending"),
            PendingAllocationSpec::default(),
            None,
        )
        .unwrap();
    let finalize_key = Idempotency {
        key: "pending-finalize".to_string(),
    };
    let finalize_options = FileMutationOptions {
        idempotency: Some(&finalize_key),
        ..FileMutationOptions::default()
    };
    let naming = AllocatedName {
        prefix: "p-",
        suffix: "",
        extension: Some("bin"),
    };
    let finalized = store
        .finalize_allocation("replay", &pending.token, naming, finalize_options)
        .unwrap();
    store.put_file("replay", "later.txt", b"later").unwrap();
    let replayed = store
        .finalize_allocation("replay", &pending.token, naming, finalize_options)
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.path, finalized.path);
    assert!(matches!(
        store.finalize_allocation(
            "replay",
            &pending.token,
            AllocatedName {
                prefix: "different-",
                ..naming
            },
            finalize_options,
        ),
        Err(StoreError::IdempotencyConflict)
    ));
}

#[test]
fn pending_fingerprint_detects_size_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("tamper", "index.html", b"site").unwrap();
    let pending = store
        .propose_allocation(
            "tamper",
            AllocationSource::Bytes(b"payload"),
            PendingAllocationSpec::default(),
            None,
        )
        .unwrap();
    {
        let mut db = store.inner.writer.lock().unwrap();
        diesel::update(pending_allocations::table.find(&pending.token))
            .set(pending_allocations::size.eq(999_i64))
            .execute(&mut *db)
            .unwrap();
    }
    assert!(matches!(
        store.finalize_allocation(
            "tamper",
            &pending.token,
            AllocatedName::default(),
            FileMutationOptions::default(),
        ),
        Err(StoreError::InvalidPendingAllocation)
    ));
}

#[test]
fn failed_allocation_authorization_never_materializes_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    let missing_hash = ContentHash::from(blake3::hash(b"missing"));
    assert!(matches!(
        store.allocate_bytes(
            "missing",
            b"missing",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        ),
        Err(StoreError::NotFound)
    ));
    assert!(!store.blob_path(missing_hash).exists());

    store.put_file("managed", "index.html", b"site").unwrap();
    let management = ManagementToken::generate().unwrap();
    {
        let mut db = store.inner.writer.lock().unwrap();
        diesel::update(sites::table.filter(sites::name.eq("managed")))
            .set((
                sites::management_hash.eq(Some(management.hash().as_bytes().as_slice())),
                sites::management_status.eq(1_i64),
            ))
            .execute(&mut *db)
            .unwrap();
    }
    let forbidden_hash = ContentHash::from(blake3::hash(b"forbidden"));
    assert!(matches!(
        store.propose_allocation(
            "managed",
            AllocationSource::Bytes(b"forbidden"),
            PendingAllocationSpec::default(),
            None,
        ),
        Err(StoreError::Unauthorized)
    ));
    assert!(!store.blob_path(forbidden_hash).exists());
}

#[test]
fn copy_and_move_file_counts_include_allocated_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("counts", "index.html", b"site").unwrap();
    store
        .allocate_bytes(
            "counts",
            b"asset",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    let (_, copied) = store
        .copy_site("counts", Some("counts-copy"), None)
        .unwrap();
    assert_eq!(copied.files, 2);
    let (_, moved) = store.move_site("counts-copy", "counts-moved").unwrap();
    assert_eq!(moved.files, 2);
}

#[test]
fn partial_expiry_keeps_site_with_allocated_survivor() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::clone(&clock) as Arc<dyn Clock>,
    )
    .unwrap();
    store
        .put_file("survivor", "temporary.txt", b"temporary")
        .unwrap();
    let survivor = store
        .allocate_bytes(
            "survivor",
            b"allocated survivor",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "survivor",
            "temporary.txt",
            Some(ExpiryPolicy::Relative {
                duration_seconds: 1,
            }),
        )
        .unwrap();

    clock.advance(1_001);
    assert_eq!(store.sweep_expired().unwrap(), 1);
    assert!(matches!(
        store.lookup("survivor", "temporary.txt"),
        Err(StoreError::NotFound)
    ));
    assert_eq!(
        node_hash(&store, "survivor", &survivor.path),
        wire_hash(&survivor.hash)
    );
}

#[test]
fn allocated_replace_and_splice_replay_after_source_relocation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("relocation-replay", "index.html", b"site")
        .unwrap();
    let allocated = store
        .allocate_bytes(
            "relocation-replay",
            b"base",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    let replace_key = Idempotency {
        key: "allocated-replace-replay".to_string(),
    };
    let replace_options = FileMutationOptions {
        idempotency: Some(&replace_key),
        ..FileMutationOptions::default()
    };
    let replaced = store
        .replace_file_content(
            "relocation-replay",
            &allocated.path,
            &allocated.hash,
            AllocationSource::Bytes(b"replaced"),
            replace_options,
        )
        .unwrap();
    assert_ne!(replaced.path, allocated.path);
    let replace_replay = store
        .replace_file_content(
            "relocation-replay",
            &allocated.path,
            &allocated.hash,
            AllocationSource::Bytes(b"replaced"),
            replace_options,
        )
        .unwrap();
    assert!(replace_replay.replayed);
    assert_eq!(replace_replay.path, replaced.path);

    let splice_key = Idempotency {
        key: "allocated-splice-replay".to_string(),
    };
    let splice_options = FileMutationOptions {
        idempotency: Some(&splice_key),
        ..FileMutationOptions::default()
    };
    let splice = [Splice {
        offset: 0,
        delete: 0,
        insert: SpliceSource::Bytes(b"x"),
    }];
    let spliced = store
        .splice_file(
            "relocation-replay",
            &replaced.path,
            &replaced.hash,
            &splice,
            splice_options,
        )
        .unwrap();
    assert_ne!(spliced.path, replaced.path);
    let splice_replay = store
        .splice_file(
            "relocation-replay",
            &replaced.path,
            &replaced.hash,
            &splice,
            splice_options,
        )
        .unwrap();
    assert!(splice_replay.replayed);
    assert_eq!(splice_replay.path, spliced.path);
}

#[test]
fn allocation_reuse_and_idempotency_require_metadata_compatibility() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("metadata-reuse", "index.html", b"site")
        .unwrap();
    store
        .allocate_bytes(
            "metadata-reuse",
            b"destination",
            AllocationSpec {
                media_type: "application/first",
                ..AllocationSpec::default()
            },
            FileMutationOptions::default(),
        )
        .unwrap();
    assert!(matches!(
        store.allocate_bytes(
            "metadata-reuse",
            b"destination",
            AllocationSpec {
                media_type: "application/different",
                ..AllocationSpec::default()
            },
            FileMutationOptions::default(),
        ),
        Err(StoreError::DestinationConflict)
    ));

    let key = Idempotency {
        key: "metadata-fingerprint".to_string(),
    };
    let options = FileMutationOptions {
        idempotency: Some(&key),
        ..FileMutationOptions::default()
    };
    store
        .allocate_bytes(
            "metadata-reuse",
            b"fingerprinted",
            AllocationSpec {
                media_type: "application/first",
                ..AllocationSpec::default()
            },
            options,
        )
        .unwrap();
    assert!(matches!(
        store.allocate_bytes(
            "metadata-reuse",
            b"fingerprinted",
            AllocationSpec {
                media_type: "application/different",
                ..AllocationSpec::default()
            },
            options,
        ),
        Err(StoreError::IdempotencyConflict)
    ));
}

#[test]
fn destination_reuse_does_not_refresh_destination_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(1_700_000_000_000));
    let store = Store::with_clock(
        dir.path().to_path_buf(),
        "http://symbol".to_string(),
        Arc::clone(&clock) as Arc<dyn Clock>,
    )
    .unwrap();
    store
        .put_file("metadata-reuse", "index.html", b"site")
        .unwrap();
    let destination = store
        .allocate_bytes(
            "metadata-reuse",
            b"destination",
            AllocationSpec {
                media_type: "application/first",
                ..AllocationSpec::default()
            },
            FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "metadata-reuse",
            &destination.path,
            Some(ExpiryPolicy::Relative {
                duration_seconds: 600,
            }),
        )
        .unwrap();
    let refreshed_before = store
        .expiry_report("metadata-reuse", &destination.path)
        .unwrap()
        .refreshed_at;
    let source = store
        .allocate_bytes(
            "metadata-reuse",
            b"source",
            AllocationSpec {
                media_type: "application/first",
                ..AllocationSpec::default()
            },
            FileMutationOptions::default(),
        )
        .unwrap();
    clock.advance(10_000);
    store
        .replace_allocated(
            "metadata-reuse",
            &source.path,
            AllocationSource::Bytes(b"destination"),
            FileMutationOptions::default(),
        )
        .unwrap();
    assert_eq!(
        store
            .expiry_report("metadata-reuse", &destination.path)
            .unwrap()
            .refreshed_at,
        refreshed_before
    );
}

#[test]
fn stats_and_site_lists_include_allocated_content_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("allocated-stats", "data.txt", b"same")
        .unwrap();
    store
        .allocate_bytes(
            "allocated-stats",
            b"same",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.sites, 1);
    assert_eq!(stats.files, 2);
    assert_eq!(stats.blobs, 1);
    assert_eq!(stats.logical_bytes, 8);
    assert_eq!(stats.bytes, 4);
    assert_eq!(stats.saved_bytes, 4);
    assert_eq!(stats.file_sizes.min, Some(4));
    assert_eq!(stats.blob_sizes.min, Some(4));

    let sites = store.list_sites().unwrap();
    assert_eq!(sites.files, 2);
    assert_eq!(sites.bytes, 8);
    assert_eq!(sites.entries[0].files, 2);
    assert_eq!(sites.entries[0].bytes, 8);
}

#[test]
fn splice_file_staging_stops_at_configured_limit() {
    let dir = tempfile::tempdir().unwrap();
    let insertion = dir.path().join("oversized");
    fs::write(&insertion, [7_u8; 32]).unwrap();
    let temporary = dir.path().join("staging");
    fs::create_dir(&temporary).unwrap();

    assert!(matches!(
        prepare_splices(
            &[Splice {
                offset: 0,
                delete: 0,
                insert: SpliceSource::File(&insertion),
            }],
            &temporary,
            8,
        ),
        Err(StoreError::SpliceResultTooLarge)
    ));
    assert!(fs::metadata(temporary.join("insert-0")).unwrap().len() <= 9);
}

#[test]
fn splice_missing_and_stale_lookups_remove_staging_directories() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .put_file("splice-cleanup", "data.txt", b"base")
        .unwrap();
    let base = node_hash(&store, "splice-cleanup", "data.txt");
    let insertion = dir.path().join("insertion");
    fs::write(&insertion, b"insert").unwrap();
    let splice = [Splice {
        offset: 0,
        delete: 0,
        insert: SpliceSource::File(&insertion),
    }];
    let splice_directories = || {
        fs::read_dir(dir.path().join("tmp"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("splice-"))
            .count()
    };
    assert_eq!(splice_directories(), 0);

    assert!(matches!(
        store.splice_file(
            "splice-cleanup",
            "missing.txt",
            &base.to_hex(),
            &splice,
            FileMutationOptions::default(),
        ),
        Err(StoreError::NotFound)
    ));
    assert_eq!(splice_directories(), 0);

    assert!(matches!(
        store.splice_file(
            "missing-site",
            "data.txt",
            &base.to_hex(),
            &splice,
            FileMutationOptions::default(),
        ),
        Err(StoreError::NotFound)
    ));
    assert_eq!(splice_directories(), 0);

    assert!(matches!(
        store.splice_file(
            "splice-cleanup",
            "data.txt",
            "stale",
            &splice,
            FileMutationOptions::default(),
        ),
        Err(StoreError::StaleContentHash(current)) if current == base
    ));
    assert_eq!(splice_directories(), 0);
}

#[test]
fn allocated_stale_and_conflicting_writes_do_not_materialize_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store.put_file("no-orphan", "index.html", b"site").unwrap();
    let allocated = store
        .allocate_bytes(
            "no-orphan",
            b"base",
            AllocationSpec::default(),
            FileMutationOptions::default(),
        )
        .unwrap();
    let stale_bytes = b"stale result";
    let stale_hash = ContentHash::from(blake3::hash(stale_bytes));
    assert!(matches!(
        store.replace_file_content(
            "no-orphan",
            &allocated.path,
            "wrong-base",
            AllocationSource::Bytes(stale_bytes),
            FileMutationOptions::default(),
        ),
        Err(StoreError::StaleContentHash(_))
    ));
    assert!(!store.blob_path(stale_hash).exists());

    let conflict_bytes = b"conflicting result";
    let conflict_hash = ContentHash::from(blake3::hash(conflict_bytes));
    let conflict_path = conflict_hash.to_hex();
    store
        .put_file("no-orphan", &conflict_path, b"occupied")
        .unwrap();
    assert!(matches!(
        store.replace_allocated(
            "no-orphan",
            &allocated.path,
            AllocationSource::Bytes(conflict_bytes),
            FileMutationOptions::default(),
        ),
        Err(StoreError::DestinationConflict)
    ));
    assert!(!store.blob_path(conflict_hash).exists());
}

#[test]
fn alias_only_archive_is_an_atomic_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("store")).unwrap();
    store
        .put_file("alias-only", "existing.txt", b"target")
        .unwrap();
    let archive = pack_tar(&[ArchiveFile::Alias {
        path: "live.txt".to_string(),
        target: "existing.txt".to_string(),
    }])
    .unwrap();
    let source = dir.path().join("aliases.tar");
    fs::write(&source, archive).unwrap();
    let (name, mutation) = store
        .publish_uploaded_archive(
            Some("alias-only"),
            Some("aliases.tar"),
            &source,
            Kind::Tar,
            PublishOptions::default(),
        )
        .unwrap();
    assert_eq!(name, "alias-only");
    assert!(mutation.changed);
    assert_eq!(
        store
            .alias("alias-only", "live.txt")
            .unwrap()
            .canonical_target,
        "existing.txt"
    );
}
