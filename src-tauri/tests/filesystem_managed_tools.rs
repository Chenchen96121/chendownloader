use std::{fs, path::Path, sync::mpsc, thread, time::Duration};

use local_media_downloader_lib::{
    application::{
        tool_bootstrap::{
            ManagedToolStore, ProbeOutput, ToolBootstrapService, ToolInventoryService,
            ToolVersionProbe,
        },
        tool_manifest::validate_manifest,
        tool_resolver::ManagedToolResolver,
    },
    domain::managed_tools::{
        ManagedToolError, ManagedToolErrorCode, ManagedToolManifest, ManifestTool, ToolName,
        ToolState, BUILD_TARGET,
    },
    infrastructure::tools::FilesystemManagedToolStore,
};

const YTDLP_BYTES: &[u8] = b"fake yt-dlp 2026.09.15";
const DENO_BYTES: &[u8] = b"fake deno 2.5.0";
const GALLERY_DL_BYTES: &[u8] = b"fake gallery-dl 1.32.15";
const FFMPEG_BYTES: &[u8] = b"fake ffmpeg 8.0.1";
const FFPROBE_BYTES: &[u8] = b"fake ffprobe 8.0.1";
const YTDLP_SHA: &str = "63de674408cec132fe939095d031da2991c5946f95e70f3b45321a2248e99741";
const DENO_SHA: &str = "704619e6f6328c54c1ca8164b3831bebb8f81be69032fb0c4cbd86530a58ce1e";
const GALLERY_DL_SHA: &str = "473a9fee6b858ab47f02187d5065e70b7fdd62891bfeb0aad9a3ce71ada31fc2";
const FFMPEG_SHA: &str = "d3f52552679add1f79534a7996292bc705d0fee4335a054ca9286061292ddec6";
const FFPROBE_SHA: &str = "f5c2c131b56a110a942df6777b65e1f39d0ad5a95b2167d53949650962060484";
const YTDLP_V2_BYTES: &[u8] = b"fake yt-dlp 2026.09.16";
const DENO_V2_BYTES: &[u8] = b"fake deno 2.5.1";
const FFMPEG_V2_BYTES: &[u8] = b"fake ffmpeg 8.0.2";
const FFPROBE_V2_BYTES: &[u8] = b"fake ffprobe 8.0.2";
const YTDLP_V2_SHA: &str = "2a69e27f5c08390be3b4545928fa28bd8ebda22886a73376ef323443936db3a3";
const DENO_V2_SHA: &str = "fd7dd9e144293190fb5a30ae20a2e5d2afe59390dd9b5fdfa5620e021f4c6bbb";
const FFMPEG_V2_SHA: &str = "3d5a02fc2bf47b73772c9272ba3c1846c44aa5ed69ae8910238c3b2a93c0e885";
const FFPROBE_V2_SHA: &str = "4eadf66be25d3bf2e2cc26c226c3c9f16f0f17b8be0daeee5c8483274e2c7625";

struct SuccessProbe;
impl ToolVersionProbe for SuccessProbe {
    fn probe(&self, tool: ToolName, _: &Path) -> Result<ProbeOutput, ManagedToolError> {
        Ok(ProbeOutput {
            exit_code: 0,
            stdout: match tool {
                ToolName::YtDlp => "2026.09.15",
                ToolName::Deno => "deno 2.5.0",
                ToolName::GalleryDl => "1.32.15",
                ToolName::Ffmpeg => "ffmpeg version 8.0.1",
                ToolName::Ffprobe => "ffprobe version 8.0.1",
            }
            .to_owned(),
            stderr: String::new(),
        })
    }
}

fn manifest() -> ManagedToolManifest {
    ManagedToolManifest {
        schema_version: 1,
        target: BUILD_TARGET.to_owned(),
        tools: [
            (ToolName::YtDlp, "2026.09.15", YTDLP_SHA),
            (ToolName::Deno, "2.5.0", DENO_SHA),
            (ToolName::GalleryDl, "1.32.15", GALLERY_DL_SHA),
            (ToolName::Ffmpeg, "8.0.1", FFMPEG_SHA),
            (ToolName::Ffprobe, "8.0.1", FFPROBE_SHA),
        ]
        .into_iter()
        .map(|(name, version, sha256)| ManifestTool {
            name,
            version: version.to_owned(),
            sha256: sha256.to_owned(),
        })
        .collect(),
    }
}

fn package(root: &Path) {
    for (tool, bytes) in [
        (ToolName::YtDlp, YTDLP_BYTES),
        (ToolName::Deno, DENO_BYTES),
        (ToolName::GalleryDl, GALLERY_DL_BYTES),
        (ToolName::Ffmpeg, FFMPEG_BYTES),
        (ToolName::Ffprobe, FFPROBE_BYTES),
    ] {
        fs::write(root.join(tool.artifact_name(BUILD_TARGET)), bytes).unwrap();
    }
}

#[test]
fn bootstrap_installs_verified_fake_artifacts() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    let inventory = ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap();
    assert_eq!(
        inventory
            .tools
            .iter()
            .map(|tool| tool.version.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("2026.09.15"),
            Some("2.5.0"),
            Some("1.32.15"),
            Some("8.0.1"),
            Some("8.0.1"),
        ]
    );
    assert!(inventory.managed_tools_configured());
}

#[test]
fn resolver_returns_only_the_digest_verified_active_artifact() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap();

    let resolved = store.resolve_verified(ToolName::Deno).unwrap();
    assert_eq!(resolved.name, ToolName::Deno);
    assert_eq!(resolved.version, "2.5.0");
    assert_eq!(fs::read(&resolved.path).unwrap(), DENO_BYTES);
    assert_eq!(
        resolved.path,
        root.path()
            .join(BUILD_TARGET)
            .join("deno/versions/2.5.0-704619e6f632")
            .join(ToolName::Deno.artifact_name(BUILD_TARGET))
    );
}

#[test]
fn resolver_rejects_an_active_artifact_changed_after_activation() {
    let (_root, store) = installed_first();
    let resolved = store.resolve_verified(ToolName::Deno).unwrap();
    fs::write(&resolved.path, b"tampered after resolution").unwrap();

    let error = store
        .resolve_verified(ToolName::Deno)
        .err()
        .expect("each resolution must recheck the active digest");
    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
}

#[test]
fn resolver_rejects_a_replaced_invalid_active_catalog() {
    let (root, store) = installed_first();
    fs::write(
        root.path().join(BUILD_TARGET).join("catalog.json"),
        b"not a managed-tool catalog",
    )
    .unwrap();

    let error = store
        .resolve_verified(ToolName::YtDlp)
        .err()
        .expect("resolution must reread and validate the active catalog");
    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
}

#[test]
fn resolver_observes_a_new_activation_instead_of_reusing_an_old_path() {
    let (_root, store) = installed_first();
    let first = store.resolve_verified(ToolName::Deno).unwrap();
    install_second(&store);

    let second = store.resolve_verified(ToolName::Deno).unwrap();
    assert_eq!(first.version, "2.5.0");
    assert_eq!(second.version, "2.5.1");
    assert_ne!(second.path, first.path);
    assert_eq!(fs::read(second.path).unwrap(), DENO_V2_BYTES);
}

fn assert_no_transactions(root: &Path) {
    let transactions = root.join(BUILD_TARGET).join("transactions");
    assert!(!transactions.exists() || fs::read_dir(transactions).unwrap().next().is_none());
}

#[test]
fn missing_ffprobe_does_not_create_catalog() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    fs::remove_file(
        source
            .path()
            .join(ToolName::Ffprobe.artifact_name(BUILD_TARGET)),
    )
    .unwrap();
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    assert_eq!(
        ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
            .bootstrap(source.path(), &manifest())
            .unwrap_err()
            .code,
        ManagedToolErrorCode::ToolMissing
    );
    assert!(!root.path().join(BUILD_TARGET).join("catalog.json").exists());
    assert_no_transactions(root.path());
}

#[test]
fn wrong_digest_does_not_create_catalog() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    fs::write(
        source
            .path()
            .join(ToolName::YtDlp.artifact_name(BUILD_TARGET)),
        b"tampered",
    )
    .unwrap();
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    assert_eq!(
        ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
            .bootstrap(source.path(), &manifest())
            .unwrap_err()
            .code,
        ManagedToolErrorCode::ToolIntegrityFailed
    );
    assert!(!root.path().join(BUILD_TARGET).join("catalog.json").exists());
    assert_no_transactions(root.path());
}

struct ThirdProbeFails;
impl ToolVersionProbe for ThirdProbeFails {
    fn probe(&self, tool: ToolName, path: &Path) -> Result<ProbeOutput, ManagedToolError> {
        if tool == ToolName::Ffprobe {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::ToolProbeFailed,
                "third probe failed",
            ));
        }
        SuccessProbe.probe(tool, path)
    }
}

#[test]
fn third_probe_failure_preserves_existing_catalog_bytes() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap();
    let catalog = root.path().join(BUILD_TARGET).join("catalog.json");
    let before = fs::read(&catalog).unwrap();
    assert_eq!(
        ToolBootstrapService::new(&store, &ThirdProbeFails, BUILD_TARGET)
            .bootstrap(source.path(), &manifest())
            .unwrap_err()
            .code,
        ManagedToolErrorCode::ToolProbeFailed
    );
    assert_eq!(fs::read(catalog).unwrap(), before);
    assert_no_transactions(root.path());
}

#[test]
fn mutated_active_artifact_invalidates_entire_inventory() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap();
    let artifact = root
        .path()
        .join(BUILD_TARGET)
        .join("yt-dlp/versions/2026.09.15-63de674408ce")
        .join(ToolName::YtDlp.artifact_name(BUILD_TARGET));
    fs::write(artifact, b"tampered active").unwrap();
    let inventory = ToolInventoryService::new(&store).inventory();
    assert_eq!(
        inventory
            .tools
            .iter()
            .map(|tool| tool.state)
            .collect::<Vec<_>>(),
        vec![ToolState::IntegrityFailed; 5]
    );
    assert_no_transactions(root.path());
}

#[test]
fn successful_bootstrap_removes_transaction_directory() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap();
    assert_no_transactions(root.path());
}

struct V2Probe;
impl ToolVersionProbe for V2Probe {
    fn probe(&self, tool: ToolName, _: &Path) -> Result<ProbeOutput, ManagedToolError> {
        Ok(ProbeOutput {
            exit_code: 0,
            stdout: match tool {
                ToolName::YtDlp => "2026.09.16",
                ToolName::Deno => "deno 2.5.1",
                ToolName::GalleryDl => "1.32.15",
                ToolName::Ffmpeg => "ffmpeg version 8.0.2",
                ToolName::Ffprobe => "ffprobe version 8.0.2",
            }
            .to_owned(),
            stderr: String::new(),
        })
    }
}

fn install_second(store: &FilesystemManagedToolStore) {
    let source = tempfile::tempdir().unwrap();
    let second = ManagedToolManifest {
        schema_version: 1,
        target: BUILD_TARGET.to_owned(),
        tools: [
            (ToolName::YtDlp, "2026.09.16", YTDLP_V2_SHA, YTDLP_V2_BYTES),
            (ToolName::Deno, "2.5.1", DENO_V2_SHA, DENO_V2_BYTES),
            (
                ToolName::GalleryDl,
                "1.32.15",
                GALLERY_DL_SHA,
                GALLERY_DL_BYTES,
            ),
            (ToolName::Ffmpeg, "8.0.2", FFMPEG_V2_SHA, FFMPEG_V2_BYTES),
            (ToolName::Ffprobe, "8.0.2", FFPROBE_V2_SHA, FFPROBE_V2_BYTES),
        ]
        .into_iter()
        .map(|(name, version, sha256, bytes)| {
            fs::write(source.path().join(name.artifact_name(BUILD_TARGET)), bytes).unwrap();
            ManifestTool {
                name,
                version: version.to_owned(),
                sha256: sha256.to_owned(),
            }
        })
        .collect(),
    };
    assert!(ToolBootstrapService::new(store, &V2Probe, BUILD_TARGET)
        .bootstrap(source.path(), &second)
        .unwrap()
        .managed_tools_configured());
}

fn installed_first() -> (tempfile::TempDir, FilesystemManagedToolStore) {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap();
    (root, store)
}

#[test]
fn matching_inventory_returns_a_ready_exact_verified_active_set() {
    let (_root, store) = installed_first();
    let expected = validate_manifest(&manifest(), BUILD_TARGET).unwrap();

    let inventory = store
        .matching_inventory(&expected)
        .expect("the exact active set must match");

    assert!(inventory.managed_tools_configured());
    assert_eq!(inventory.tools[0].version.as_deref(), Some("2026.09.15"));
}

#[test]
fn matching_inventory_rejects_same_versions_with_a_different_digest() {
    let (_root, store) = installed_first();
    let mut different = manifest();
    different.tools[0].sha256 =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
    let different = validate_manifest(&different, BUILD_TARGET).unwrap();

    assert!(store.matching_inventory(&different).is_none());
}

#[test]
fn matching_inventory_rejects_a_damaged_active_artifact() {
    let (root, store) = installed_first();
    fs::write(
        root.path()
            .join(BUILD_TARGET)
            .join("yt-dlp/versions/2026.09.15-63de674408ce")
            .join(ToolName::YtDlp.artifact_name(BUILD_TARGET)),
        b"damaged active artifact",
    )
    .unwrap();
    let expected = validate_manifest(&manifest(), BUILD_TARGET).unwrap();

    assert!(store.matching_inventory(&expected).is_none());
}

#[test]
fn bootstrap_repairs_a_damaged_regular_active_artifact_from_verified_staging() {
    let (root, store) = installed_first();
    let active = store.resolve_verified(ToolName::YtDlp).unwrap().path;
    let catalog = root.path().join(BUILD_TARGET).join("catalog.json");
    let before = fs::read(&catalog).unwrap();
    fs::write(&active, b"damaged active artifact").unwrap();
    let source = tempfile::tempdir().unwrap();
    package(source.path());

    let inventory = ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .expect("a verified local bundle must repair a damaged ordinary file");

    assert!(inventory.managed_tools_configured());
    assert_eq!(fs::read(active).unwrap(), YTDLP_BYTES);
    assert_eq!(fs::read(catalog).unwrap(), before);
    assert_no_transactions(root.path());
}

#[test]
fn bootstrap_refuses_a_directory_at_the_damaged_artifact_path() {
    let (_root, store) = installed_first();
    let active = store.resolve_verified(ToolName::YtDlp).unwrap().path;
    fs::remove_file(&active).unwrap();
    fs::create_dir(&active).unwrap();
    fs::write(active.join("retained.txt"), b"must not be deleted").unwrap();
    let source = tempfile::tempdir().unwrap();
    package(source.path());
    let error = ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap_err();
    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
    assert_eq!(
        fs::read(active.join("retained.txt")).unwrap(),
        b"must not be deleted"
    );
}

#[test]
fn matching_inventory_does_not_trust_an_edited_catalog_and_artifact() {
    let (root, store) = installed_first();
    let target = root.path().join(BUILD_TARGET);
    let edited_bytes = b"locally substituted yt-dlp";
    let edited_sha = "a42d6d009b86000bdbda5db267c9ea0b94de6f8ac9746a1e9fa89461d0d69ba7";
    let mut edited = manifest();
    edited.tools[0].sha256 = edited_sha.to_owned();
    let edited_path = target
        .join(format!("yt-dlp/versions/2026.09.15-{}", &edited_sha[..12]))
        .join(ToolName::YtDlp.artifact_name(BUILD_TARGET));
    fs::create_dir_all(edited_path.parent().unwrap()).unwrap();
    fs::write(&edited_path, edited_bytes).unwrap();
    fs::write(
        target.join("catalog.json"),
        serde_json::to_vec(&edited).unwrap(),
    )
    .unwrap();
    let embedded = validate_manifest(&manifest(), BUILD_TARGET).unwrap();

    assert!(store.matching_inventory(&embedded).is_none());
}

#[test]
fn second_activation_preserves_first_catalog_bytes() {
    let (root, store) = installed_first();
    let target = root.path().join(BUILD_TARGET);
    let first = fs::read(target.join("catalog.json")).unwrap();
    install_second(&store);
    assert_eq!(
        fs::read(target.join("catalog.previous.json")).unwrap(),
        first
    );
    assert_no_transactions(root.path());
}

#[test]
fn rollback_restores_first_versions_and_retains_second_catalog() {
    let (root, store) = installed_first();
    install_second(&store);
    let target = root.path().join(BUILD_TARGET);
    let second = fs::read(target.join("catalog.json")).unwrap();
    let inventory = ToolInventoryService::new(&store)
        .rollback()
        .expect("previous catalog must restore");
    assert_eq!(
        inventory
            .tools
            .iter()
            .map(|tool| tool.version.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("2026.09.15"),
            Some("2.5.0"),
            Some("1.32.15"),
            Some("8.0.1"),
            Some("8.0.1"),
        ]
    );
    assert!(inventory.managed_tools_configured());
    assert_eq!(
        fs::read(target.join("catalog.previous.json")).unwrap(),
        second
    );
}

#[test]
fn missing_active_recovers_valid_previous_set() {
    let (root, store) = installed_first();
    install_second(&store);
    let target = root.path().join(BUILD_TARGET);
    fs::remove_file(target.join("catalog.json")).unwrap();
    let inventory = ToolInventoryService::new(&store).inventory();
    assert_eq!(inventory.tools[0].version.as_deref(), Some("2026.09.15"));
    assert!(inventory.managed_tools_configured());
    assert!(target.join("catalog.json").exists());
}

#[test]
fn malformed_pending_catalog_is_never_selected() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join(BUILD_TARGET);
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("catalog.pending.json"), b"malformed pending").unwrap();
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    let inventory = ToolInventoryService::new(&store).inventory();
    assert!(!inventory.managed_tools_configured());
    assert!(inventory
        .tools
        .iter()
        .all(|tool| tool.state == ToolState::Missing));
    assert!(!target.join("catalog.json").exists());
}

#[test]
fn invalid_active_and_previous_fail_closed() {
    let (root, store) = installed_first();
    let target = root.path().join(BUILD_TARGET);
    fs::write(target.join("catalog.json"), b"invalid active").unwrap();
    fs::write(target.join("catalog.previous.json"), b"invalid previous").unwrap();
    assert_eq!(
        ToolInventoryService::new(&store)
            .inventory()
            .tools
            .iter()
            .map(|tool| tool.state)
            .collect::<Vec<_>>(),
        vec![ToolState::IntegrityFailed; 5]
    );
}

#[test]
fn recovery_does_not_rotate_invalid_active_over_valid_previous() {
    let (root, store) = installed_first();
    install_second(&store);
    let target = root.path().join(BUILD_TARGET);
    let previous = fs::read(target.join("catalog.previous.json")).unwrap();
    fs::write(target.join("catalog.json"), b"invalid active").unwrap();
    let inventory = ToolInventoryService::new(&store).inventory();
    assert_eq!(inventory.tools[0].version.as_deref(), Some("2026.09.15"));
    assert_eq!(
        fs::read(target.join("catalog.previous.json")).unwrap(),
        previous
    );
}

#[test]
fn recovery_discards_stale_pending_without_selecting_its_content() {
    for invalid_active in [false, true] {
        let (root, store) = installed_first();
        install_second(&store);
        let target = root.path().join(BUILD_TARGET);
        let previous = fs::read(target.join("catalog.previous.json")).unwrap();
        if invalid_active {
            fs::write(target.join("catalog.json"), b"malformed active").unwrap();
        } else {
            fs::remove_file(target.join("catalog.json")).unwrap();
        }
        fs::write(
            target.join("catalog.pending.json"),
            b"malformed stale pending",
        )
        .unwrap();
        let inventory = ToolInventoryService::new(&store).inventory();
        assert!(inventory.managed_tools_configured());
        assert_eq!(inventory.tools[0].version.as_deref(), Some("2026.09.15"));
        assert_eq!(fs::read(target.join("catalog.json")).unwrap(), previous);
        assert_eq!(
            fs::read(target.join("catalog.previous.json")).unwrap(),
            previous
        );
        assert!(!target.join("catalog.pending.json").exists());
    }
}

#[test]
fn second_activation_discards_stale_previous_pending() {
    let (root, store) = installed_first();
    let target = root.path().join(BUILD_TARGET);
    let first = fs::read(target.join("catalog.json")).unwrap();
    fs::write(
        target.join("catalog.previous.pending.json"),
        b"malformed stale previous pending",
    )
    .unwrap();
    install_second(&store);
    assert_eq!(
        fs::read(target.join("catalog.previous.json")).unwrap(),
        first
    );
    assert!(!target.join("catalog.previous.pending.json").exists());
}

#[test]
fn invalid_previous_artifact_prevents_rollback_without_catalog_changes() {
    let (root, store) = installed_first();
    install_second(&store);
    let target = root.path().join(BUILD_TARGET);
    let active = fs::read(target.join("catalog.json")).unwrap();
    let previous = fs::read(target.join("catalog.previous.json")).unwrap();
    fs::write(
        target
            .join("yt-dlp/versions/2026.09.15-63de674408ce")
            .join(ToolName::YtDlp.artifact_name(BUILD_TARGET)),
        b"tampered previous",
    )
    .unwrap();
    assert!(ToolInventoryService::new(&store).rollback().is_err());
    assert_eq!(fs::read(target.join("catalog.json")).unwrap(), active);
    assert_eq!(
        fs::read(target.join("catalog.previous.json")).unwrap(),
        previous
    );
}

// The break these tests catch: any lifecycle entry point bypassing the same-root OS lock
// can replace/delete another writer's pending bytes and return before that writer releases it.
fn assert_another_instance_waits_for_catalog_lock(operation: &'static str) {
    let (root, store) = installed_first();
    install_second(&store);
    let target = root.path().join(BUILD_TARGET);
    if operation == "recovery" {
        fs::remove_file(target.join("catalog.json")).unwrap();
    }
    fs::write(
        target.join("catalog.pending.json"),
        b"first writer pending bytes",
    )
    .unwrap();
    let active_before = fs::read(target.join("catalog.json")).ok();
    let previous_before = fs::read(target.join("catalog.previous.json")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(target.join("catalog.lock"))
        .unwrap();
    lock.lock().unwrap();
    let worker_root = root.path().to_owned();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let another_store = FilesystemManagedToolStore::new(worker_root, BUILD_TARGET);
        started_tx.send(()).unwrap();
        let result = match operation {
            "activate" => {
                let source = tempfile::tempdir().unwrap();
                package(source.path());
                ToolBootstrapService::new(&another_store, &SuccessProbe, BUILD_TARGET)
                    .bootstrap(source.path(), &manifest())
            }
            "rollback" => ToolInventoryService::new(&another_store).rollback(),
            "recovery" => Ok(ToolInventoryService::new(&another_store).inventory()),
            _ => unreachable!(),
        };
        done_tx.send(result).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = done_rx.recv_timeout(Duration::from_millis(300));
    let unchanged = fs::read(target.join("catalog.json")).ok() == active_before
        && fs::read(target.join("catalog.previous.json")).unwrap() == previous_before
        && fs::read(target.join("catalog.pending.json"))
            .ok()
            .as_deref()
            == Some(b"first writer pending bytes".as_slice());
    drop(lock);
    let (returned_early, result) = match early {
        Ok(result) => (true, result),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            (false, done_rx.recv_timeout(Duration::from_secs(5)).unwrap())
        }
        Err(error) => panic!("lifecycle worker disconnected: {error}"),
    };
    worker.join().unwrap();
    assert!(
        !returned_early,
        "another store instance returned before catalog lock release during {operation}"
    );
    assert!(
        unchanged,
        "catalog or pending evidence changed while catalog lock was held during {operation}"
    );
    assert!(result.unwrap().managed_tools_configured());
    assert!(
        target.join("catalog.lock").is_file(),
        "persistent lock file must not be removed"
    );
}

#[test]
fn another_instance_activation_waits_for_catalog_lock() {
    assert_another_instance_waits_for_catalog_lock("activate");
}

#[test]
fn another_instance_rollback_waits_for_catalog_lock() {
    assert_another_instance_waits_for_catalog_lock("rollback");
}

#[test]
fn another_instance_recovery_waits_for_catalog_lock() {
    assert_another_instance_waits_for_catalog_lock("recovery");
}

#[test]
fn clean_inventory_remains_missing_with_persistent_lock_file() {
    let root = tempfile::tempdir().unwrap();
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    let inventory = ToolInventoryService::new(&store).inventory();
    assert!(inventory
        .tools
        .iter()
        .all(|tool| tool.state == ToolState::Missing));
    assert!(!inventory.managed_tools_configured());
}

#[test]
fn dangling_immutable_destination_reparse_point_is_rejected_without_overwrite() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    package(source.path());
    let destination = root
        .path()
        .join(BUILD_TARGET)
        .join("yt-dlp")
        .join("versions")
        .join("2026.09.15-63de674408ce")
        .join(ToolName::YtDlp.artifact_name(BUILD_TARGET));
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let missing_target = root.path().join("nonexistent-symlink-target");
    #[cfg(windows)]
    {
        // Test-only dangling directory junction: no Developer Mode or symlink privilege needed.
        // Both command arguments must be bounded beneath this test's owned absolute root.
        let owned_root = fs::canonicalize(root.path()).unwrap();
        assert!(destination.is_absolute() && missing_target.is_absolute());
        assert!(fs::canonicalize(destination.parent().unwrap())
            .unwrap()
            .starts_with(&owned_root));
        assert!(fs::canonicalize(missing_target.parent().unwrap())
            .unwrap()
            .starts_with(&owned_root));
        assert!(!missing_target.exists());
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&destination)
            .arg(&missing_target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "temporary junction fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(&missing_target, &destination).unwrap();
    let evidence = fs::symlink_metadata(&destination).unwrap();
    assert!(evidence.file_type().is_symlink() || !evidence.is_file());
    let original_link = fs::read_link(&destination).unwrap();
    assert!(
        !destination.try_exists().unwrap(),
        "fixture must be dangling"
    );
    let store = FilesystemManagedToolStore::new(root.path().to_owned(), BUILD_TARGET);
    let error = ToolBootstrapService::new(&store, &SuccessProbe, BUILD_TARGET)
        .bootstrap(source.path(), &manifest())
        .unwrap_err();
    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
    let retained = fs::symlink_metadata(&destination).unwrap();
    assert!(retained.file_type().is_symlink() || !retained.is_file());
    assert_eq!(fs::read_link(&destination).unwrap(), original_link);
    assert!(!root.path().join(BUILD_TARGET).join("catalog.json").exists());
    assert_no_transactions(root.path());
}
