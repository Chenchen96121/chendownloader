use std::{
    env, fs,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use local_media_downloader_lib::{
    application::{
        portable_package::{TrustedToolPackageSource, EMBEDDED_TOOL_MANIFEST},
        tool_bootstrap::{ManagedToolStore, ProbeOutput, ToolVersionProbe},
        tool_provisioning::{OfflineToolProvisioner, ProvisioningMode, ToolProvisioner},
        tool_resolver::ManagedToolResolver,
    },
    domain::managed_tools::{ManagedToolError, ToolName, BUILD_TARGET},
    infrastructure::{
        process::WindowsProcessSupervisor,
        tools::{
            FilesystemManagedToolStore, FilesystemPortableToolPackage, NativeToolVersionProbe,
        },
    },
};

struct CountingProbe {
    native: NativeToolVersionProbe<Arc<WindowsProcessSupervisor>>,
    calls: AtomicUsize,
}

impl ToolVersionProbe for CountingProbe {
    fn probe(&self, tool: ToolName, candidate: &Path) -> Result<ProbeOutput, ManagedToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.native.probe(tool, candidate)
    }
}

#[test]
#[ignore = "requires the exact extracted portable tool package; executes local version probes only"]
fn real_portable_startup_move_repair_and_corruption_boundaries() {
    let input = env::var_os("LMD_REAL_TOOL_PACKAGE").expect("real package setting missing");
    let runtime = tempfile::tempdir().unwrap();
    let bundle = runtime.path().join("中文 解压目录");
    let package = bundle.join("resources/managed-tools");
    fs::create_dir_all(&package).unwrap();
    for entry in fs::read_dir(input).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        fs::copy(entry.path(), package.join(entry.file_name())).unwrap();
    }
    let probe = CountingProbe {
        native: NativeToolVersionProbe::new(Arc::new(WindowsProcessSupervisor)),
        calls: AtomicUsize::new(0),
    };
    let store = FilesystemManagedToolStore::new(runtime.path().join("用户 本地数据"), BUILD_TARGET);
    let source = FilesystemPortableToolPackage::from_executable(
        &bundle.join("app.exe"),
        BUILD_TARGET,
        Arc::from(EMBEDDED_TOOL_MANIFEST),
    );
    let expected = source.load_manifest().unwrap();
    let required_tool_count = ToolName::required().len();
    let provisioner = OfflineToolProvisioner::new(&source, &store, &probe, BUILD_TARGET);
    assert!(provisioner
        .provision(ProvisioningMode::Startup)
        .unwrap()
        .managed_tools_configured());
    assert_eq!(probe.calls.load(Ordering::SeqCst), required_tool_count);
    assert!(provisioner
        .provision(ProvisioningMode::Startup)
        .unwrap()
        .managed_tools_configured());
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        required_tool_count,
        "matching startup must not reprobe"
    );

    let moved = runtime.path().join("移动后 portable app");
    fs::rename(&bundle, &moved).unwrap();
    let moved_source = FilesystemPortableToolPackage::from_executable(
        &moved.join("app.exe"),
        BUILD_TARGET,
        Arc::from(EMBEDDED_TOOL_MANIFEST),
    );
    let moved_provisioner =
        OfflineToolProvisioner::new(&moved_source, &store, &probe, BUILD_TARGET);
    assert!(moved_provisioner
        .provision(ProvisioningMode::Startup)
        .unwrap()
        .managed_tools_configured());
    let active = store.resolve_verified(ToolName::GalleryDl).unwrap().path;
    fs::write(&active, b"damaged QA tool").unwrap();
    assert!(store.matching_inventory(&expected).is_none());
    assert!(moved_provisioner
        .provision(ProvisioningMode::Startup)
        .unwrap()
        .managed_tools_configured());
    assert_eq!(probe.calls.load(Ordering::SeqCst), required_tool_count * 2);
    assert!(store.matching_inventory(&expected).is_some());

    let catalog_path = runtime
        .path()
        .join("用户 本地数据")
        .join(BUILD_TARGET)
        .join("catalog.json");
    let catalog_before = fs::read(&catalog_path).unwrap();
    let bundled_gallery_dl = moved
        .join("resources/managed-tools")
        .join(ToolName::GalleryDl.artifact_name(BUILD_TARGET));
    fs::write(bundled_gallery_dl, b"damaged QA bundle").unwrap();
    assert!(moved_provisioner
        .provision(ProvisioningMode::Startup)
        .unwrap()
        .managed_tools_configured());
    assert!(moved_provisioner
        .provision(ProvisioningMode::Repair)
        .is_err());
    assert_eq!(fs::read(&catalog_path).unwrap(), catalog_before);
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        required_tool_count * 2,
        "corrupt bundle must not execute"
    );
    assert!(store.matching_inventory(&expected).is_some());

    let fresh_store =
        FilesystemManagedToolStore::new(runtime.path().join("全新失败数据"), BUILD_TARGET);
    assert!(
        OfflineToolProvisioner::new(&moved_source, &fresh_store, &probe, BUILD_TARGET)
            .provision(ProvisioningMode::Startup)
            .is_err()
    );
    assert!(!runtime
        .path()
        .join("全新失败数据")
        .join(BUILD_TARGET)
        .join("catalog.json")
        .exists());
    let target_root = catalog_path.parent().unwrap();
    fs::write(
        target_root.join("catalog.pending.json"),
        b"interrupted invalid pending",
    )
    .unwrap();
    fs::remove_file(&catalog_path).unwrap();
    assert!(store.inventory().managed_tools_configured());
    assert!(!target_root.join("catalog.pending.json").exists());
    assert_eq!(fs::read(catalog_path).unwrap(), catalog_before);
}
