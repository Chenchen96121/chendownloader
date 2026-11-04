use local_media_downloader_lib::{
    domain::{
        managed_tools::{ToolName, ToolState},
        media::AnalysisErrorCode,
    },
    infrastructure::tiktok::tool::TikTokToolResolver,
};
use sha2::{Digest, Sha256};

fn manifest(bytes: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"protocolVersion":1,"target":"x86_64-pc-windows-msvc", "version":"1.1.0","artifact":"tiktok-core-x86_64-pc-windows-msvc.exe","sha256":format!("{:x}",Sha256::digest(bytes))})).unwrap()
}

#[test]
fn missing_or_tampered_tiktok_component_does_not_change_core_tool_set() {
    let root = tempfile::tempdir().unwrap();
    let resolver = TikTokToolResolver::new(root.path().to_owned(), &manifest(b"fixture")).unwrap();
    assert_eq!(resolver.status().state, ToolState::Missing);
    assert_eq!(ToolName::required().len(), 5);
    std::fs::write(
        root.path().join("tiktok-core-x86_64-pc-windows-msvc.exe"),
        b"fixture",
    )
    .unwrap();
    assert_eq!(resolver.status().state, ToolState::Ready);
    std::fs::write(
        root.path().join("tiktok-core-x86_64-pc-windows-msvc.exe"),
        b"changed",
    )
    .unwrap();
    assert_eq!(resolver.status().state, ToolState::IntegrityFailed);
}

#[test]
fn manifest_rejects_wrong_identity_protocol_path_and_extra_fields() {
    let root = tempfile::tempdir().unwrap();
    for (key, value) in [
        ("artifact", serde_json::json!("../untrusted.exe")),
        ("protocolVersion", serde_json::json!(2)),
        ("target", serde_json::json!("other")),
        ("version", serde_json::json!("0.0.0")),
        ("sha256", serde_json::json!("bad")),
        ("script", serde_json::json!("untrusted")),
    ] {
        let mut object: serde_json::Value = serde_json::from_slice(&manifest(b"fixture")).unwrap();
        object[key] = value;
        assert!(matches!(
            TikTokToolResolver::new(
                root.path().to_owned(),
                &serde_json::to_vec(&object).unwrap()
            ),
            Err(AnalysisErrorCode::ToolIntegrityFailed)
        ));
    }
}
