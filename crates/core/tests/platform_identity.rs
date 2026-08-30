//! Machine identity and the on-destination manifest, exercised through the
//! public API against a real (temporary) destination root.
//!
//! Nothing here touches the registry, installs a service, or writes outside a
//! per-test temporary directory.

use chrono::{Duration, Utc};
use superbackup_core::model::{MachineIdentity, MANIFEST_DIR};
use superbackup_core::paths::Paths;
use superbackup_core::platform::identity;
use uuid::Uuid;

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!(
            "sb-plat-{tag}-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn identity_named(label: &str) -> MachineIdentity {
    let id = Uuid::new_v4();
    MachineIdentity {
        id,
        label: label.to_string(),
        hostname: label.to_string(),
        os: "windows".into(),
        os_version: "Windows 11 Pro 24H2 (build 26200.1)".into(),
        arch: "x86_64".into(),
        username: "andreas".into(),
        slug: identity::slug_for(label, &id),
        created_at: Utc::now(),
    }
}

#[test]
fn the_machine_id_is_persisted_and_stable_across_runs() {
    let dir = TempDir::new("machineid");
    let paths = Paths::rooted_at(dir.path(), false);

    let first = identity::load_or_create_id(&paths).expect("first run mints an id");
    let second = identity::load_or_create_id(&paths).expect("second run reuses it");
    assert_eq!(first, second);

    // Version 4: random, not derived from any hardware serial.
    assert_eq!(first.get_version_num(), 4, "the id must not be a hardware fingerprint");

    // A corrupt id file must not stop the program starting.
    std::fs::write(paths.config_dir.join(identity::MACHINE_ID_FILE), b"not-a-uuid").expect("write");
    let third = identity::load_or_create_id(&paths).expect("a corrupt id file is replaced");
    assert_ne!(third, first);
}

#[test]
fn manifest_round_trips_and_preserves_first_seen() {
    let dir = TempDir::new("manifest");
    let me = identity_named("Studio PC");
    let t0 = Utc::now() - Duration::days(30);

    let written = identity::write_manifest_at(dir.path(), &me, t0).expect("first write");
    assert_eq!(written.first_seen, t0);
    assert_eq!(written.last_seen, t0);
    assert_eq!(written.slug, me.slug);
    assert_eq!(written.superbackup_version, superbackup_core::VERSION);

    let t1 = Utc::now();
    let again = identity::write_manifest_at(dir.path(), &me, t1).expect("second write");
    assert_eq!(again.first_seen, t0, "first_seen must survive later writes");
    assert_eq!(again.last_seen, t1);

    let listed = identity::list_machines(dir.path()).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, me.id);
    assert_eq!(listed[0].first_seen, t0);
    assert!(listed[0].days_since_seen(t1) == 0);
}

#[test]
fn the_layout_matches_what_model_rs_documents() {
    let dir = TempDir::new("layout");
    let me = identity_named("Laptop");
    identity::write_manifest(dir.path(), &me).expect("write");

    let manifest_dir = dir.path().join(MANIFEST_DIR);
    assert!(manifest_dir.is_dir(), "the reserved folder must be named _superbackup");
    assert!(manifest_dir.join("machines").join(format!("{}.json", me.id)).is_file());
    assert!(manifest_dir.join("README.txt").is_file());
    assert_eq!(identity::machine_root(dir.path(), &me), dir.path().join(&me.slug));
}

#[test]
fn two_machines_with_the_same_label_get_separate_folders_and_both_are_listed() {
    let dir = TempDir::new("collision");
    let a = identity_named("Laptop");
    let b = identity_named("Laptop");
    assert_ne!(a.slug, b.slug);

    identity::write_manifest(dir.path(), &a).expect("write a");
    identity::write_manifest(dir.path(), &b).expect("write b");

    let listed = identity::list_machines(dir.path()).expect("list");
    assert_eq!(listed.len(), 2, "both PCs must appear: {listed:?}");
    let slugs: Vec<&str> = listed.iter().map(|m| m.slug.as_str()).collect();
    assert!(slugs.contains(&a.slug.as_str()));
    assert!(slugs.contains(&b.slug.as_str()));

    assert_eq!(
        identity::describe_occupancy(&listed, &a.id),
        "Holds backups from this PC and 1 other"
    );
    assert!(listed.iter().any(|m| m.is_foreign(&a.id)));
}

#[test]
fn renaming_keeps_the_folder_and_records_the_old_name() {
    let dir = TempDir::new("rename");
    let mut me = identity_named("DESKTOP-8H2K1L");
    let folder = me.slug.clone();
    identity::write_manifest(dir.path(), &me).expect("first write");

    let event = identity::rename(&mut me, "Andreas' Studio PC").expect("a rename event");
    assert_eq!(event.kind, "machine.renamed");
    assert_eq!(me.slug, folder, "the folder must not move");

    let record = identity::write_manifest(dir.path(), &me).expect("second write");
    assert_eq!(record.slug, folder);
    assert_eq!(record.label, "Andreas' Studio PC");
    assert_eq!(
        record.previous_labels.last().map(|l| l.label.as_str()),
        Some("DESKTOP-8H2K1L"),
        "the old name must still be discoverable by a human browsing the drive"
    );

    let readme = std::fs::read_to_string(identity::readme_path(dir.path())).expect("README");
    assert!(readme.contains("Andreas' Studio PC"));
    assert!(readme.contains("DESKTOP-8H2K1L"), "the README should mention the rename");
}

#[test]
fn the_readme_explains_the_folder_to_a_human() {
    let dir = TempDir::new("readme");
    identity::write_manifest(dir.path(), &identity_named("Studio")).expect("write");
    let readme = std::fs::read_to_string(identity::readme_path(dir.path())).expect("README");

    // The whole point of the feature: someone opening a shared drive can tell
    // what this is, whose it is, that it is encrypted, and what deleting costs.
    assert!(readme.contains("superbackup"));
    assert!(readme.contains("Studio"));
    assert!(readme.to_lowercase().contains("encrypted"));
    assert!(readme.to_lowercase().contains("kopia"));
    assert!(readme.to_lowercase().contains("delet"));
    assert!(!readme.contains("<"), "no markup: this is opened in Notepad");
}

#[test]
fn a_destination_nobody_has_used_lists_no_machines() {
    let dir = TempDir::new("empty");
    assert!(identity::list_machines(dir.path()).expect("list").is_empty());
    assert!(
        identity::list_machines(&dir.path().join("does-not-exist")).expect("list").is_empty(),
        "a missing destination is an empty one, not an error"
    );
}

#[test]
fn a_corrupt_record_does_not_hide_the_others() {
    let dir = TempDir::new("corrupt");
    let good = identity_named("Good");
    identity::write_manifest(dir.path(), &good).expect("write");
    std::fs::write(
        identity::machines_dir(dir.path()).join("00000000-0000-0000-0000-000000000000.json"),
        b"{ truncated",
    )
    .expect("write");

    let listed = identity::list_machines(dir.path()).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, good.id);
}

#[test]
fn a_record_written_by_a_newer_version_keeps_its_unknown_fields() {
    let dir = TempDir::new("forward");
    let me = identity_named("Future");
    identity::write_manifest(dir.path(), &me).expect("write");

    let path = identity::record_path(dir.path(), &me.id);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    value["quantum_dedup_ratio"] = serde_json::json!(1.5);
    std::fs::write(&path, serde_json::to_vec_pretty(&value).expect("json")).expect("write");

    // An older build re-writing the record must not silently drop the field.
    identity::write_manifest(dir.path(), &me).expect("rewrite");
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    assert_eq!(
        after["quantum_dedup_ratio"],
        serde_json::json!(1.5),
        "unknown fields must survive a round trip through an older reader"
    );
}

#[test]
fn forgetting_a_machine_removes_only_its_record() {
    let dir = TempDir::new("forget");
    let a = identity_named("A");
    let b = identity_named("B");
    identity::write_manifest(dir.path(), &a).expect("write a");
    identity::write_manifest(dir.path(), &b).expect("write b");

    assert!(identity::forget_machine(dir.path(), &a.id).expect("forget"));
    assert!(
        !identity::forget_machine(dir.path(), &a.id).expect("forget again"),
        "forgetting twice is not an error"
    );
    let listed = identity::list_machines(dir.path()).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, b.id);
}

#[test]
fn detecting_this_machine_produces_a_specific_os_version() {
    let dir = TempDir::new("detect");
    let paths = Paths::rooted_at(dir.path(), false);
    let me = identity::detect(&paths).expect("detect");

    assert_eq!(me.os, std::env::consts::OS);
    assert_eq!(me.arch, std::env::consts::ARCH);
    assert!(!me.hostname.is_empty());
    assert!(!me.username.is_empty());
    assert!(me.slug.ends_with(&me.id.simple().to_string()[..8]));
    assert!(
        me.os_version.len() > std::env::consts::OS.len(),
        "\"{}\" is not a useful version string",
        me.os_version
    );
    if cfg!(windows) {
        assert!(me.os_version.starts_with("Windows"), "{}", me.os_version);
        assert!(
            me.os_version.contains("build "),
            "the real build number is required, got {}",
            me.os_version
        );
    }
}
