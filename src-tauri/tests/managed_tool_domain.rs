use local_media_downloader_lib::{
    application::tool_manifest::validate_manifest,
    domain::managed_tools::{
        ManagedToolErrorCode, ManagedToolManifest, ManifestTool, ToolInventory, ToolName, ToolState,
    },
};

const TARGET: &str = "x86_64-pc-windows-msvc";

#[test]
fn artifact_names_are_derived_from_the_closed_tool_set() {
    assert_eq!(
        ToolName::YtDlp.artifact_name(TARGET),
        "yt-dlp-x86_64-pc-windows-msvc.exe"
    );
    assert_eq!(
        ToolName::Deno.artifact_name(TARGET),
        "deno-x86_64-pc-windows-msvc.exe"
    );
    assert_eq!(
        ToolName::GalleryDl.artifact_name(TARGET),
        "gallery-dl-x86_64-pc-windows-msvc.exe"
    );
    assert_eq!(
        ToolName::Ffmpeg.artifact_name(TARGET),
        "ffmpeg-x86_64-pc-windows-msvc.exe"
    );
    assert_eq!(
        ToolName::Ffprobe.artifact_name(TARGET),
        "ffprobe-x86_64-pc-windows-msvc.exe"
    );
}

#[test]
fn missing_inventory_is_stable_and_not_configured() {
    let inventory = ToolInventory::missing(TARGET);
    assert_eq!(
        inventory
            .tools
            .iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>(),
        vec![
            ToolName::YtDlp,
            ToolName::Deno,
            ToolName::GalleryDl,
            ToolName::Ffmpeg,
            ToolName::Ffprobe,
        ]
    );
    assert!(inventory
        .tools
        .iter()
        .all(|tool| tool.state == ToolState::Missing && tool.version.is_none()));
    assert!(!inventory.managed_tools_configured());
}

#[test]
fn malformed_ready_inventory_is_not_configured() {
    let mut inventory = ToolInventory::missing(TARGET);
    for tool in &mut inventory.tools {
        tool.state = ToolState::Ready;
        tool.version = Some("verified".to_owned());
    }
    inventory.tools[3].name = ToolName::YtDlp;
    assert!(!inventory.managed_tools_configured());
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

#[test]
fn managed_tools_have_stable_public_names_and_required_positions() {
    assert_eq!(serde_json::to_string(&ToolName::Deno).unwrap(), "\"deno\"");
    assert_eq!(
        serde_json::to_string(&ToolName::GalleryDl).unwrap(),
        "\"gallery-dl\""
    );
    assert_eq!(
        ToolName::required(),
        [
            ToolName::YtDlp,
            ToolName::Deno,
            ToolName::GalleryDl,
            ToolName::Ffmpeg,
            ToolName::Ffprobe,
        ]
    );
}

#[test]
fn gallery_dl_manifest_version_is_exactly_pinned() {
    validate_manifest(&valid_manifest(), TARGET).expect("1.32.15 must validate");

    for invalid in ["1.32.14", "1.32.16", "v1.32.15", "1.32.15.0"] {
        let mut manifest = valid_manifest();
        manifest
            .tools
            .iter_mut()
            .find(|tool| tool.name == ToolName::GalleryDl)
            .unwrap()
            .version = invalid.to_owned();
        let error = validate_manifest(&manifest, TARGET)
            .expect_err("gallery-dl must remain on the audited release");
        assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
    }
}

#[test]
fn a_three_tool_manifest_is_not_a_ready_set() {
    let mut manifest = valid_manifest();
    manifest.tools.retain(|tool| tool.name != ToolName::Deno);
    let error = validate_manifest(&manifest, TARGET).expect_err("Deno is required");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn deno_versions_are_bounded_three_part_decimals() {
    for valid in ["0.0.0", "2.5.0", "99999.99999.99999"] {
        let mut manifest = valid_manifest();
        manifest
            .tools
            .iter_mut()
            .find(|tool| tool.name == ToolName::Deno)
            .unwrap()
            .version = valid.to_owned();
        validate_manifest(&manifest, TARGET).expect("bounded Deno semver must validate");
    }

    for invalid in [
        "2.5",
        "2.5.0.1",
        "100000.1.1",
        "1.100000.1",
        "1.1.100000",
        "1.-1.1",
        "v2.5.0",
    ] {
        let mut manifest = valid_manifest();
        manifest
            .tools
            .iter_mut()
            .find(|tool| tool.name == ToolName::Deno)
            .unwrap()
            .version = invalid.to_owned();
        let error = validate_manifest(&manifest, TARGET)
            .expect_err("unbounded or non-three-part Deno version must fail");
        assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
    }
}

#[test]
fn manifest_validation_normalizes_tools_to_the_required_order() {
    let mut manifest = valid_manifest();
    manifest.tools.reverse();
    let validated = validate_manifest(&manifest, TARGET).expect("manifest must validate");
    assert_eq!(
        validated
            .tools()
            .iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>(),
        ToolName::required()
    );
}

#[test]
fn manifest_validation_rejects_a_wrong_target() {
    let mut manifest = valid_manifest();
    manifest.target = "x86_64-unknown-linux-gnu".to_owned();
    let error = validate_manifest(&manifest, TARGET).expect_err("wrong target must fail");
    assert_eq!(error.code, ManagedToolErrorCode::UnsupportedTarget);
}

#[test]
fn manifest_validation_rejects_duplicate_tool_membership() {
    let mut manifest = valid_manifest();
    manifest.tools[3].name = ToolName::YtDlp;
    let error = validate_manifest(&manifest, TARGET).expect_err("duplicate tool must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn manifest_validation_rejects_an_uppercase_digest() {
    let mut manifest = valid_manifest();
    manifest.tools[0].sha256 =
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned();
    let error = validate_manifest(&manifest, TARGET).expect_err("uppercase digest must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn manifest_validation_rejects_a_short_digest() {
    let mut manifest = valid_manifest();
    manifest.tools[0].sha256 =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
    let error = validate_manifest(&manifest, TARGET).expect_err("short digest must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn manifest_validation_rejects_an_invalid_general_tool_version() {
    let mut manifest = valid_manifest();
    manifest.tools[2].version = "../8.0.1".to_owned();
    let error = validate_manifest(&manifest, TARGET).expect_err("invalid version must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn manifest_validation_rejects_a_non_padded_yt_dlp_version() {
    let mut manifest = valid_manifest();
    manifest.tools[0].version = "2026.6.09".to_owned();
    let error =
        validate_manifest(&manifest, TARGET).expect_err("non-padded yt-dlp version must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn manifest_validation_rejects_an_invalid_yt_dlp_month() {
    let mut manifest = valid_manifest();
    manifest.tools[0].version = "2026.13.01".to_owned();
    let error = validate_manifest(&manifest, TARGET).expect_err("invalid yt-dlp month must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn manifest_validation_rejects_an_old_yt_dlp_version() {
    let mut manifest = valid_manifest();
    manifest.tools[0].version = "2026.06.08".to_owned();
    let error = validate_manifest(&manifest, TARGET).expect_err("old yt-dlp version must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}

#[test]
fn manifest_validation_rejects_mismatched_ffmpeg_versions() {
    let mut manifest = valid_manifest();
    manifest.tools[3].version = "8.0.0".to_owned();
    let error = validate_manifest(&manifest, TARGET).expect_err("mismatched versions must fail");
    assert_eq!(error.code, ManagedToolErrorCode::InvalidToolManifest);
}
