use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use local_media_downloader_lib::{
    application::{
        portable_package::{TrustedToolPackage, TrustedToolPackageSource},
        tool_bootstrap::{
            ManagedToolStore, ProbeOutput, StagedPackage, StagedTool, ToolVersionProbe,
        },
        tool_manifest::{validate_manifest, ValidatedManifest},
        tool_provisioning::{OfflineToolProvisioner, ProvisioningMode, ToolProvisioner},
    },
    domain::managed_tools::{
        ManagedToolError, ManagedToolErrorCode, ManagedToolManifest, ManifestTool, ToolInventory,
        ToolName, ToolState, ToolStatus,
    },
};

const TARGET: &str = "x86_64-pc-windows-msvc";

struct RecordingSource {
    manifest: ValidatedManifest,
    package_root: PathBuf,
    manifest_loads: AtomicUsize,
    package_loads: AtomicUsize,
}

impl RecordingSource {
    fn new(manifest: ValidatedManifest) -> Self {
        Self {
            manifest,
            package_root: PathBuf::from("trusted bundle"),
            manifest_loads: AtomicUsize::new(0),
            package_loads: AtomicUsize::new(0),
        }
    }
}

impl TrustedToolPackageSource for RecordingSource {
    fn load_manifest(&self) -> Result<ValidatedManifest, ManagedToolError> {
        self.manifest_loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.manifest.clone())
    }

    fn load_package(
        &self,
        manifest: &ValidatedManifest,
    ) -> Result<TrustedToolPackage, ManagedToolError> {
        self.package_loads.fetch_add(1, Ordering::SeqCst);
        if manifest != &self.manifest {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::ToolIntegrityFailed,
                "requested manifest changed",
            ));
        }
        Ok(TrustedToolPackage::new(
            manifest.clone(),
            self.package_root.clone(),
        ))
    }
}

#[derive(Default)]
struct RecordingStore {
    active: RefCell<Option<ValidatedManifest>>,
    active_catalog: RefCell<Vec<u8>>,
    previous_catalog: RefCell<Vec<u8>>,
    stage_calls: Cell<usize>,
    activation_calls: Cell<usize>,
    discard_calls: Cell<usize>,
}

impl RecordingStore {
    fn with_active(manifest: ValidatedManifest) -> Self {
        Self {
            active_catalog: RefCell::new(catalog_bytes(&manifest)),
            active: RefCell::new(Some(manifest)),
            ..Self::default()
        }
    }
}

impl ManagedToolStore for RecordingStore {
    fn matching_inventory(&self, manifest: &ValidatedManifest) -> Option<ToolInventory> {
        match self.active.borrow().as_ref() {
            Some(active) if active == manifest => Some(ready_inventory(active)),
            _ => None,
        }
    }

    fn stage(
        &self,
        package_root: &Path,
        manifest: &ValidatedManifest,
    ) -> Result<StagedPackage, ManagedToolError> {
        self.stage_calls.set(self.stage_calls.get() + 1);
        Ok(StagedPackage {
            manifest: manifest.clone(),
            transaction_dir: package_root.join("transaction"),
            tools: manifest
                .tools()
                .iter()
                .map(|tool| StagedTool {
                    name: tool.name,
                    version: tool.version.clone(),
                    sha256: tool.sha256.clone(),
                    path: package_root.join(tool.name.artifact_name(TARGET)),
                })
                .collect(),
        })
    }

    fn activate(&self, staged: &StagedPackage) -> Result<(), ManagedToolError> {
        self.activation_calls.set(self.activation_calls.get() + 1);
        *self.previous_catalog.borrow_mut() = self.active_catalog.borrow().clone();
        *self.active_catalog.borrow_mut() = catalog_bytes(&staged.manifest);
        self.active.replace(Some(staged.manifest.clone()));
        Ok(())
    }

    fn discard(&self, _staged: &StagedPackage) {
        self.discard_calls.set(self.discard_calls.get() + 1);
    }

    fn inventory(&self) -> ToolInventory {
        self.active
            .borrow()
            .as_ref()
            .map(ready_inventory)
            .unwrap_or_else(|| ToolInventory::missing(TARGET))
    }

    fn rollback(&self) -> Result<ToolInventory, ManagedToolError> {
        Ok(self.inventory())
    }
}

#[derive(Default)]
struct RecordingProbe {
    calls: RefCell<Vec<ToolName>>,
    fail_on: Option<ToolName>,
}

impl RecordingProbe {
    fn failing(tool: ToolName) -> Self {
        Self {
            fail_on: Some(tool),
            ..Self::default()
        }
    }
}

impl ToolVersionProbe for RecordingProbe {
    fn probe(&self, tool: ToolName, _candidate: &Path) -> Result<ProbeOutput, ManagedToolError> {
        self.calls.borrow_mut().push(tool);
        if self.fail_on == Some(tool) {
            return Err(ManagedToolError::new(
                ManagedToolErrorCode::ToolProbeFailed,
                "fixture probe failure",
            ));
        }
        Ok(ProbeOutput {
            exit_code: 0,
            stdout: match tool {
                ToolName::YtDlp => "2026.08.19",
                ToolName::Deno => "deno 2.9.7",
                ToolName::GalleryDl => "1.32.15",
                ToolName::Ffmpeg => "ffmpeg version 9.0.2",
                ToolName::Ffprobe => "ffprobe version 9.0.2",
            }
            .to_owned(),
            stderr: String::new(),
        })
    }
}

#[test]
fn startup_reuses_an_exact_verified_active_set_without_loading_bundle_files() {
    let manifest = release_manifest();
    let source = RecordingSource::new(manifest.clone());
    let store = RecordingStore::with_active(manifest);
    let probe = RecordingProbe::default();

    let inventory = OfflineToolProvisioner::new(&source, &store, &probe, TARGET)
        .provision(ProvisioningMode::Startup)
        .unwrap();

    assert!(inventory.managed_tools_configured());
    assert_eq!(source.manifest_loads.load(Ordering::SeqCst), 1);
    assert_eq!(source.package_loads.load(Ordering::SeqCst), 0);
    assert!(probe.calls.borrow().is_empty());
    assert_eq!(store.stage_calls.get(), 0);
    assert_eq!(store.activation_calls.get(), 0);
}

#[test]
fn startup_bootstraps_a_missing_set_from_the_bundle() {
    let manifest = release_manifest();
    let source = RecordingSource::new(manifest);
    let store = RecordingStore::default();
    let probe = RecordingProbe::default();

    let inventory = OfflineToolProvisioner::new(&source, &store, &probe, TARGET)
        .provision(ProvisioningMode::Startup)
        .unwrap();

    assert!(inventory.managed_tools_configured());
    assert_eq!(source.manifest_loads.load(Ordering::SeqCst), 1);
    assert_eq!(source.package_loads.load(Ordering::SeqCst), 1);
    assert_eq!(probe.calls.borrow().as_slice(), ToolName::required());
    assert_eq!(store.stage_calls.get(), 1);
    assert_eq!(store.activation_calls.get(), 1);
    assert_eq!(store.discard_calls.get(), 1);
}

#[test]
fn repair_always_validates_and_reprovisions_the_bundle() {
    let manifest = release_manifest();
    let source = RecordingSource::new(manifest.clone());
    let store = RecordingStore::with_active(manifest);
    let probe = RecordingProbe::default();

    OfflineToolProvisioner::new(&source, &store, &probe, TARGET)
        .provision(ProvisioningMode::Repair)
        .unwrap();

    assert_eq!(source.manifest_loads.load(Ordering::SeqCst), 1);
    assert_eq!(source.package_loads.load(Ordering::SeqCst), 1);
    assert_eq!(probe.calls.borrow().len(), 5);
    assert_eq!(store.stage_calls.get(), 1);
    assert_eq!(store.activation_calls.get(), 1);
}

#[test]
fn failed_repair_preserves_matching_active_catalog_bytes() {
    let manifest = release_manifest();
    let source = RecordingSource::new(manifest.clone());
    let store = RecordingStore::with_active(manifest);
    let before_active = store.active_catalog.borrow().clone();
    let before_previous = store.previous_catalog.borrow().clone();
    let probe = RecordingProbe::failing(ToolName::Ffprobe);

    let error = OfflineToolProvisioner::new(&source, &store, &probe, TARGET)
        .provision(ProvisioningMode::Repair)
        .unwrap_err();

    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
    assert_eq!(*store.active_catalog.borrow(), before_active);
    assert_eq!(*store.previous_catalog.borrow(), before_previous);
    assert_eq!(store.activation_calls.get(), 0);
    assert_eq!(store.discard_calls.get(), 1);
}

#[test]
fn failed_new_package_preserves_the_previous_application_catalog() {
    let source = RecordingSource::new(release_manifest());
    let store = RecordingStore::with_active(previous_manifest());
    let before_active = store.active_catalog.borrow().clone();
    let before_previous = store.previous_catalog.borrow().clone();
    let probe = RecordingProbe::failing(ToolName::Ffmpeg);

    let error = OfflineToolProvisioner::new(&source, &store, &probe, TARGET)
        .provision(ProvisioningMode::Startup)
        .unwrap_err();

    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
    assert_eq!(*store.active_catalog.borrow(), before_active);
    assert_eq!(*store.previous_catalog.borrow(), before_previous);
    assert_eq!(store.activation_calls.get(), 0);
    assert_eq!(store.discard_calls.get(), 1);
}

fn release_manifest() -> ValidatedManifest {
    manifest("2026.08.19", "2.9.7", "9.0.2", ['a', 'b', 'c', 'd', 'e'])
}

fn previous_manifest() -> ValidatedManifest {
    manifest("2026.08.18", "2.9.6", "9.0.1", ['f', '0', '1', '2', '3'])
}

fn manifest(
    yt_dlp_version: &str,
    deno_version: &str,
    ffmpeg_version: &str,
    digest_chars: [char; 5],
) -> ValidatedManifest {
    let manifest = ManagedToolManifest {
        schema_version: 1,
        target: TARGET.to_owned(),
        tools: ToolName::required()
            .into_iter()
            .zip([
                yt_dlp_version,
                deno_version,
                "1.32.15",
                ffmpeg_version,
                ffmpeg_version,
            ])
            .zip(digest_chars)
            .map(|((name, version), digest)| ManifestTool {
                name,
                version: version.to_owned(),
                sha256: digest.to_string().repeat(64),
            })
            .collect(),
    };
    validate_manifest(&manifest, TARGET).unwrap()
}

fn ready_inventory(manifest: &ValidatedManifest) -> ToolInventory {
    ToolInventory {
        target: manifest.target().to_owned(),
        tools: manifest
            .tools()
            .iter()
            .map(|tool| ToolStatus {
                name: tool.name,
                state: ToolState::Ready,
                version: Some(tool.version.clone()),
            })
            .collect(),
    }
}

fn catalog_bytes(manifest: &ValidatedManifest) -> Vec<u8> {
    serde_json::to_vec(&manifest.clone().into_manifest()).unwrap()
}
