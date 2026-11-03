use std::{fs, path::Path, sync::Arc};

use local_media_downloader_lib::{
    application::portable_package::{TrustedToolPackageSource, EMBEDDED_TOOL_MANIFEST},
    domain::managed_tools::{ManagedToolErrorCode, ManagedToolManifest, ToolName, BUILD_TARGET},
    infrastructure::tools::FilesystemPortableToolPackage,
};

const EXPECTED: [(ToolName, &str, &str); 5] = [
    (
        ToolName::YtDlp,
        "2026.08.19",
        "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a",
    ),
    (
        ToolName::Deno,
        "2.9.7",
        "e020f3e232bd16e33768dee528e5983349c962952051ced0a5d58ad42f5d9b33",
    ),
    (
        ToolName::GalleryDl,
        "1.32.15",
        "d6c8ac8ffdc9194e90787225e70d0a7c9eeed23dd41c9574e235632b6f9593a4",
    ),
    (
        ToolName::Ffmpeg,
        "9.0.2-chendownloader-d7643a0fe3",
        "5ba6d0b142ab2ba524e42c16f0fa1d4586fe1ee3099b743997a7f44910d5513d",
    ),
    (
        ToolName::Ffprobe,
        "9.0.2-chendownloader-d7643a0fe3",
        "03b88ec9e8fd8946d0587658842c26e081dafe4a308604d991dcdf631c16fdae",
    ),
];

#[test]
fn embedded_release_manifest_pins_the_exact_five_tool_set() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    let executable = temp.path().join("Local Media Downloader.exe");
    let source = FilesystemPortableToolPackage::from_executable(
        &executable,
        BUILD_TARGET,
        Arc::from(EMBEDDED_TOOL_MANIFEST),
    );

    let manifest = source.load_manifest().expect("embedded manifest");
    assert_eq!(manifest.target(), "x86_64-pc-windows-msvc");
    assert_eq!(
        manifest
            .tools()
            .iter()
            .map(|tool| (tool.name, tool.version.as_str(), tool.sha256.as_str()))
            .collect::<Vec<_>>(),
        EXPECTED
    );
}

#[test]
fn portable_root_is_derived_only_from_the_executable_parent() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    let app_root = temp.path().join("含 空格").join("portable");
    let executable = app_root.join("Local Media Downloader.exe");
    write_package(&app_root, EMBEDDED_TOOL_MANIFEST);
    let source = FilesystemPortableToolPackage::from_executable(
        &executable,
        BUILD_TARGET,
        Arc::from(EMBEDDED_TOOL_MANIFEST),
    );
    let manifest = source.load_manifest().expect("embedded manifest");

    let package = source.load_package(&manifest).expect("portable package");

    assert_eq!(
        package.package_root(),
        fs::canonicalize(app_root.join("resources").join("managed-tools"))
            .expect("canonical package root")
    );
}

#[test]
fn visible_manifest_must_equal_the_embedded_bytes() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    let executable = temp.path().join("Local Media Downloader.exe");
    let mut changed = EMBEDDED_TOOL_MANIFEST.to_vec();
    changed.push(b' ');
    write_package(temp.path(), &changed);
    let source = FilesystemPortableToolPackage::from_executable(
        &executable,
        BUILD_TARGET,
        Arc::from(EMBEDDED_TOOL_MANIFEST),
    );
    let manifest = source.load_manifest().expect("embedded manifest");

    assert!(source.load_package(&manifest).is_err());
}

#[test]
fn load_package_rejects_a_missing_required_tool() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    write_package(temp.path(), EMBEDDED_TOOL_MANIFEST);
    fs::remove_file(
        temp.path()
            .join("resources/managed-tools")
            .join(ToolName::Deno.artifact_name(BUILD_TARGET)),
    )
    .expect("remove one tool");
    let source = source(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let manifest = source.load_manifest().expect("embedded manifest");

    let error = source
        .load_package(&manifest)
        .err()
        .expect("missing tool must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolMissing);
}

#[test]
fn load_package_rejects_an_unexpected_direct_child() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    write_package(temp.path(), EMBEDDED_TOOL_MANIFEST);
    fs::write(
        temp.path().join("resources/managed-tools/unexpected.exe"),
        b"fixture",
    )
    .expect("write unexpected entry");
    let source = source(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let manifest = source.load_manifest().expect("embedded manifest");

    let error = source
        .load_package(&manifest)
        .err()
        .expect("unexpected entry must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
}

#[test]
fn load_package_rejects_a_nested_entry() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    write_package(temp.path(), EMBEDDED_TOOL_MANIFEST);
    fs::create_dir(temp.path().join("resources/managed-tools").join("nested"))
        .expect("write nested entry");
    let source = source(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let manifest = source.load_manifest().expect("embedded manifest");

    let error = source
        .load_package(&manifest)
        .err()
        .expect("nested entry must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
}

#[test]
fn load_package_rejects_a_renamed_tool() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    write_package(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let root = temp.path().join("resources/managed-tools");
    fs::rename(
        root.join(ToolName::Ffmpeg.artifact_name(BUILD_TARGET)),
        root.join("ffmpeg-renamed.exe"),
    )
    .expect("rename tool");
    let source = source(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let manifest = source.load_manifest().expect("embedded manifest");

    let error = source
        .load_package(&manifest)
        .err()
        .expect("renamed tool must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolMissing);
}

#[test]
fn load_manifest_rejects_a_wrong_target() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    let mut manifest: ManagedToolManifest =
        serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).expect("fixture manifest");
    manifest.target = "aarch64-pc-windows-msvc".to_owned();
    let bytes = serde_json::to_vec(&manifest).expect("serialize manifest");
    let source = source(temp.path(), &bytes);

    let error = source.load_manifest().expect_err("wrong target");

    assert_eq!(error.code, ManagedToolErrorCode::UnsupportedTarget);
}

#[test]
fn load_manifest_rejects_a_malformed_digest() {
    let temp = tempfile::tempdir().expect("temporary portable root");
    let mut manifest: ManagedToolManifest =
        serde_json::from_slice(EMBEDDED_TOOL_MANIFEST).expect("fixture manifest");
    manifest.tools[0].sha256 = "ABC".to_owned();
    let bytes = serde_json::to_vec(&manifest).expect("serialize manifest");
    let source = source(temp.path(), &bytes);

    let error = source.load_manifest().expect_err("malformed digest");

    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[cfg(windows)]
#[test]
fn load_package_rejects_a_symlinked_tool_when_windows_allows_creating_one() {
    use std::os::windows::fs::symlink_file;

    let temp = tempfile::tempdir().expect("temporary portable root");
    write_package(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let root = temp.path().join("resources/managed-tools");
    let tool = root.join(ToolName::YtDlp.artifact_name(BUILD_TARGET));
    fs::remove_file(&tool).expect("remove ordinary tool");
    let outside = temp.path().join("outside.exe");
    fs::write(&outside, b"outside").expect("write outside file");
    if symlink_file(&outside, &tool).is_err() {
        return;
    }
    let source = source(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let manifest = source.load_manifest().expect("embedded manifest");

    let error = source
        .load_package(&manifest)
        .err()
        .expect("symlinked tool must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
}

#[cfg(windows)]
#[test]
fn load_package_rejects_a_symlinked_package_root_when_windows_allows_creating_one() {
    use std::os::windows::fs::symlink_dir;

    let temp = tempfile::tempdir().expect("temporary portable root");
    let outside = temp.path().join("outside-package");
    write_package_at(&outside, EMBEDDED_TOOL_MANIFEST);
    let resources = temp.path().join("resources");
    fs::create_dir(&resources).expect("create resources directory");
    if symlink_dir(&outside, resources.join("managed-tools")).is_err() {
        return;
    }
    let source = source(temp.path(), EMBEDDED_TOOL_MANIFEST);
    let manifest = source.load_manifest().expect("embedded manifest");

    let error = source
        .load_package(&manifest)
        .err()
        .expect("symlinked package root must fail");

    assert_eq!(error.code, ManagedToolErrorCode::ToolIntegrityFailed);
}

fn source(app_root: &Path, manifest: &[u8]) -> FilesystemPortableToolPackage {
    FilesystemPortableToolPackage::from_executable(
        &app_root.join("Local Media Downloader.exe"),
        BUILD_TARGET,
        Arc::from(manifest),
    )
}

fn write_package(app_root: &Path, manifest: &[u8]) {
    let package_root = app_root.join("resources").join("managed-tools");
    write_package_at(&package_root, manifest);
}

fn write_package_at(package_root: &Path, manifest: &[u8]) {
    fs::create_dir_all(package_root).expect("create package root");
    fs::write(package_root.join("manifest.json"), manifest).expect("write visible manifest");
    for tool in ToolName::required() {
        fs::write(
            package_root.join(tool.artifact_name(BUILD_TARGET)),
            b"fixture",
        )
        .expect("write tool fixture");
    }
}
