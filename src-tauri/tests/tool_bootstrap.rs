use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::Path,
};

use local_media_downloader_lib::{
    application::{
        tool_bootstrap::{
            parse_reported_version, ManagedToolStore, ProbeOutput, StagedPackage, StagedTool,
            ToolBootstrapService, ToolInventoryService, ToolVersionProbe,
        },
        tool_manifest::{validate_manifest, ValidatedManifest},
    },
    domain::managed_tools::{
        ManagedToolError, ManagedToolErrorCode, ManagedToolManifest, ManifestTool, ToolInventory,
        ToolName, ToolState,
    },
};

const TARGET: &str = "x86_64-pc-windows-msvc";

fn success(stdout: &str) -> ProbeOutput {
    ProbeOutput {
        exit_code: 0,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

#[test]
fn parses_reported_versions_from_the_managed_tool_outputs() {
    assert_eq!(
        parse_reported_version(ToolName::YtDlp, &success("2026.09.15\n")).unwrap(),
        "2026.09.15"
    );
    assert_eq!(
        parse_reported_version(
            ToolName::Deno,
            &success("deno 2.9.7 (stable, release, x86_64-pc-windows-msvc)\nv8 15.0\n")
        )
        .unwrap(),
        "2.9.7"
    );
    assert_eq!(
        parse_reported_version(ToolName::GalleryDl, &success("1.32.15\n")).unwrap(),
        "1.32.15"
    );
    assert_eq!(
        parse_reported_version(
            ToolName::Ffmpeg,
            &success("ffmpeg version 8.0.1 Copyright\n")
        )
        .unwrap(),
        "8.0.1"
    );
    assert_eq!(
        parse_reported_version(
            ToolName::Ffprobe,
            &success("ffprobe version 8.0.1 Copyright\n")
        )
        .unwrap(),
        "8.0.1"
    );
}

#[test]
fn deno_probe_requires_the_first_line_to_start_with_its_name_and_version() {
    for output in ["Deno 2.5.0\n", "v8 14.0\ndeno 2.5.0\n", "deno\n"] {
        let error = parse_reported_version(ToolName::Deno, &success(output))
            .expect_err("unexpected Deno banners must fail");
        assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
    }
}

#[test]
fn rejects_nonzero_probe_exit_without_exposing_probe_output() {
    let error = parse_reported_version(
        ToolName::YtDlp,
        &ProbeOutput {
            exit_code: 1,
            stdout: "sensitive stdout".to_owned(),
            stderr: "sensitive stderr".to_owned(),
        },
    )
    .expect_err("a nonzero probe exit must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
    assert!(!error.detail.contains("sensitive"));
}

#[test]
fn rejects_empty_probe_stdout() {
    let error = parse_reported_version(ToolName::YtDlp, &success(" \n\t\n"))
        .expect_err("an empty version report must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
}

#[test]
fn rejects_a_non_ffmpeg_version_prefix() {
    let error = parse_reported_version(ToolName::Ffmpeg, &success("ffmpeg build 8.0.1\n"))
        .expect_err("an unexpected ffmpeg banner must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
}

#[test]
fn rejects_lossy_utf8_markers_even_after_a_valid_version_token() {
    let error = parse_reported_version(
        ToolName::Ffmpeg,
        &success("ffmpeg version 8.0.1 Copyright \u{fffd}\n"),
    )
    .expect_err("lossy UTF-8 output must never be accepted");

    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
}

#[derive(Default)]
struct FakeStore {
    stage_error: RefCell<Option<ManagedToolError>>,
    activation_error: RefCell<Option<ManagedToolError>>,
    staged_manifest: RefCell<Option<ValidatedManifest>>,
    staged_version_overrides: RefCell<HashMap<ToolName, String>>,
    active: RefCell<Option<StagedPackage>>,
    discard_count: Cell<usize>,
    activation_count: Cell<usize>,
}

impl FakeStore {
    fn with_stage_error(code: ManagedToolErrorCode) -> Self {
        Self {
            stage_error: RefCell::new(Some(ManagedToolError::new(code, "stage failed"))),
            ..Self::default()
        }
    }

    fn active_package(&self) -> Option<StagedPackage> {
        self.active.borrow().clone()
    }

    fn with_staged_version(self, tool: ToolName, version: &str) -> Self {
        self.staged_version_overrides
            .borrow_mut()
            .insert(tool, version.to_owned());
        self
    }

    fn with_staged_manifest(self, manifest: ManagedToolManifest) -> Self {
        self.staged_manifest.replace(Some(
            validate_manifest(&manifest, TARGET).expect("replacement manifest must be valid"),
        ));
        self
    }
}

impl ManagedToolStore for FakeStore {
    fn matching_inventory(&self, manifest: &ValidatedManifest) -> Option<ToolInventory> {
        match self.active_package() {
            Some(package) if package.manifest == *manifest => Some(self.inventory()),
            _ => None,
        }
    }

    fn stage(
        &self,
        package_root: &Path,
        manifest: &ValidatedManifest,
    ) -> Result<StagedPackage, ManagedToolError> {
        if let Some(error) = self.stage_error.borrow_mut().take() {
            return Err(error);
        }

        let manifest = self
            .staged_manifest
            .borrow()
            .clone()
            .unwrap_or_else(|| manifest.clone());

        Ok(StagedPackage {
            manifest: manifest.clone(),
            transaction_dir: package_root.join("transaction"),
            tools: manifest
                .tools()
                .iter()
                .map(|tool| StagedTool {
                    name: tool.name,
                    version: self
                        .staged_version_overrides
                        .borrow()
                        .get(&tool.name)
                        .cloned()
                        .unwrap_or_else(|| tool.version.clone()),
                    sha256: tool.sha256.clone(),
                    path: package_root.join(tool.name.artifact_name(manifest.target())),
                })
                .collect(),
        })
    }

    fn activate(&self, staged: &StagedPackage) -> Result<(), ManagedToolError> {
        self.activation_count.set(self.activation_count.get() + 1);
        if let Some(error) = self.activation_error.borrow_mut().take() {
            return Err(error);
        }
        self.active.replace(Some(staged.clone()));
        Ok(())
    }

    fn discard(&self, _staged: &StagedPackage) {
        self.discard_count.set(self.discard_count.get() + 1);
    }

    fn inventory(&self) -> ToolInventory {
        let Some(package) = self.active_package() else {
            return ToolInventory::missing(TARGET);
        };
        ToolInventory {
            target: package.manifest.target().to_owned(),
            tools: package
                .tools
                .iter()
                .map(
                    |tool| local_media_downloader_lib::domain::managed_tools::ToolStatus {
                        name: tool.name,
                        state: ToolState::Ready,
                        version: Some(tool.version.clone()),
                    },
                )
                .collect(),
        }
    }

    fn rollback(&self) -> Result<ToolInventory, ManagedToolError> {
        self.active.replace(None);
        Ok(self.inventory())
    }
}

#[derive(Default)]
struct FakeProbe {
    outputs: RefCell<HashMap<ToolName, Result<ProbeOutput, ManagedToolError>>>,
    calls: Cell<usize>,
}

impl FakeProbe {
    fn with_output(self, tool: ToolName, output: ProbeOutput) -> Self {
        self.outputs.borrow_mut().insert(tool, Ok(output));
        self
    }

    fn with_error(self, tool: ToolName, code: ManagedToolErrorCode) -> Self {
        self.outputs
            .borrow_mut()
            .insert(tool, Err(ManagedToolError::new(code, "probe failed")));
        self
    }
}

impl ToolVersionProbe for FakeProbe {
    fn probe(&self, tool: ToolName, _candidate: &Path) -> Result<ProbeOutput, ManagedToolError> {
        self.calls.set(self.calls.get() + 1);
        self.outputs
            .borrow()
            .get(&tool)
            .cloned()
            .unwrap_or_else(|| {
                Err(ManagedToolError::new(
                    ManagedToolErrorCode::ToolMissing,
                    "missing probe",
                ))
            })
    }
}

fn valid_manifest() -> ManagedToolManifest {
    ManagedToolManifest {
        schema_version: 1,
        target: TARGET.to_owned(),
        tools: vec![
            ManifestTool {
                name: ToolName::YtDlp,
                version: "2026.09.15".to_owned(),
                sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
            },
            ManifestTool {
                name: ToolName::Deno,
                version: "2.5.0".to_owned(),
                sha256: "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                    .to_owned(),
            },
            ManifestTool {
                name: ToolName::GalleryDl,
                version: "1.32.15".to_owned(),
                sha256: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                    .to_owned(),
            },
            ManifestTool {
                name: ToolName::Ffmpeg,
                version: "8.0.1".to_owned(),
                sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_owned(),
            },
            ManifestTool {
                name: ToolName::Ffprobe,
                version: "8.0.1".to_owned(),
                sha256: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    .to_owned(),
            },
        ],
    }
}

fn bootstrap_service<'a>(store: &'a FakeStore, probe: &'a FakeProbe) -> ToolBootstrapService<'a> {
    ToolBootstrapService::new(store, probe, TARGET)
}

fn exact_version_probe() -> FakeProbe {
    FakeProbe::default()
        .with_output(ToolName::YtDlp, success("2026.09.15\n"))
        .with_output(ToolName::Deno, success("deno 2.5.0\nv8 14.0\n"))
        .with_output(ToolName::GalleryDl, success("1.32.15\n"))
        .with_output(
            ToolName::Ffmpeg,
            success("ffmpeg version 8.0.1 Copyright\n"),
        )
        .with_output(
            ToolName::Ffprobe,
            success("ffprobe version 8.0.1 Copyright\n"),
        )
}

#[test]
fn bootstrap_activates_only_after_all_required_versions_match() {
    let store = FakeStore::default();
    let probe = exact_version_probe();

    let inventory = bootstrap_service(&store, &probe)
        .bootstrap(Path::new("package"), &valid_manifest())
        .expect("matching staged tools must activate");

    assert!(inventory.managed_tools_configured());
    assert!(store.active_package().is_some());
    assert_eq!(store.activation_count.get(), 1);
    assert_eq!(store.discard_count.get(), 1);
}

#[test]
fn bootstrap_validated_still_enforces_the_service_target() {
    let store = FakeStore::default();
    let probe = exact_version_probe();
    let mut other = valid_manifest();
    other.target = "aarch64-pc-windows-msvc".to_owned();
    let other = validate_manifest(&other, "aarch64-pc-windows-msvc").unwrap();

    let error = bootstrap_service(&store, &probe)
        .bootstrap_validated(Path::new("package"), &other)
        .expect_err("prevalidated manifests must not bypass the service target");

    assert_eq!(error.code, ManagedToolErrorCode::UnsupportedTarget);
    assert_eq!(store.activation_count.get(), 0);
    assert_eq!(probe.calls.get(), 0);
}

#[test]
fn bootstrap_discards_the_transaction_when_a_later_probe_fails() {
    let store = FakeStore::default();
    let probe =
        exact_version_probe().with_error(ToolName::Ffprobe, ManagedToolErrorCode::ToolProbeFailed);

    let error = bootstrap_service(&store, &probe)
        .bootstrap(Path::new("package"), &valid_manifest())
        .expect_err("a probe failure must prevent activation");

    assert_eq!(error.code, ManagedToolErrorCode::ToolProbeFailed);
    assert!(store.active_package().is_none());
    assert_eq!(store.activation_count.get(), 0);
    assert_eq!(store.discard_count.get(), 1);
}

#[test]
fn bootstrap_rejects_a_version_mismatch_without_activating() {
    let store = FakeStore::default();
    let probe = exact_version_probe().with_output(
        ToolName::Ffmpeg,
        success("ffmpeg version 8.0.2 Copyright\n"),
    );

    let error = bootstrap_service(&store, &probe)
        .bootstrap(Path::new("package"), &valid_manifest())
        .expect_err("a version mismatch must prevent activation");

    assert_eq!(error.code, ManagedToolErrorCode::ToolVersionMismatch);
    assert!(store.active_package().is_none());
    assert_eq!(store.activation_count.get(), 0);
    assert_eq!(store.discard_count.get(), 1);
}

#[test]
fn bootstrap_compares_reported_versions_with_the_validated_manifest() {
    let store = FakeStore::default().with_staged_version(ToolName::Ffmpeg, "8.0.2");
    let probe = exact_version_probe().with_output(
        ToolName::Ffmpeg,
        success("ffmpeg version 8.0.2 Copyright\n"),
    );

    let error = bootstrap_service(&store, &probe)
        .bootstrap(Path::new("package"), &valid_manifest())
        .expect_err("a staged metadata change must not override the validated manifest");

    assert_eq!(error.code, ManagedToolErrorCode::ToolVersionMismatch);
    assert!(store.active_package().is_none());
    assert_eq!(store.activation_count.get(), 0);
    assert_eq!(store.discard_count.get(), 1);
}

#[test]
fn bootstrap_rejects_a_staged_package_with_a_different_valid_manifest() {
    let mut replacement_manifest = valid_manifest();
    replacement_manifest.tools[3].version = "8.0.2".to_owned();
    replacement_manifest.tools[4].version = "8.0.2".to_owned();
    let store = FakeStore::default().with_staged_manifest(replacement_manifest);
    let probe = FakeProbe::default()
        .with_output(ToolName::YtDlp, success("2026.09.15\n"))
        .with_output(ToolName::Deno, success("deno 2.5.0\nv8 14.0\n"))
        .with_output(ToolName::GalleryDl, success("1.32.15\n"))
        .with_output(
            ToolName::Ffmpeg,
            success("ffmpeg version 8.0.2 Copyright\n"),
        )
        .with_output(
            ToolName::Ffprobe,
            success("ffprobe version 8.0.2 Copyright\n"),
        );

    let error = bootstrap_service(&store, &probe)
        .bootstrap(Path::new("package"), &valid_manifest())
        .expect_err("a staged package must not replace the requested validated manifest");

    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
    assert!(store.active_package().is_none());
    assert_eq!(store.activation_count.get(), 0);
    assert_eq!(store.discard_count.get(), 1);
}

#[test]
fn bootstrap_does_not_probe_when_staging_fails_integrity_checks() {
    let store = FakeStore::with_stage_error(ManagedToolErrorCode::ToolIntegrityFailed);
    let probe = exact_version_probe();

    let error = bootstrap_service(&store, &probe)
        .bootstrap(Path::new("package"), &valid_manifest())
        .expect_err("a staging integrity failure must stop bootstrap");

    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
    assert_eq!(probe.calls.get(), 0);
    assert!(store.active_package().is_none());
    assert_eq!(store.discard_count.get(), 0);
}

#[test]
fn inventory_service_exposes_the_store_inventory_and_rollback_result() {
    let store = FakeStore::default();
    let probe = exact_version_probe();
    bootstrap_service(&store, &probe)
        .bootstrap(Path::new("package"), &valid_manifest())
        .expect("setup must activate the package");
    let service = ToolInventoryService::new(&store);

    assert!(service.inventory().managed_tools_configured());
    assert!(!service
        .rollback()
        .expect("rollback must return the resulting inventory")
        .managed_tools_configured());
}
