use local_media_downloader_lib::{
    application::{
        network_profile::NetworkProfile, process::CancellationToken, x_source::XRequestContext,
    },
    domain::media::AnalysisErrorCode,
    infrastructure::x::{
        hls_local::{
            materialize_hls, HlsByteRange, HlsMaterializationRequest, HlsReadMode,
            HlsSegmentTransport, HlsTransferError,
        },
        manifest::{build_hls_snapshot, HlsManifestTransport},
    },
};
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
struct Manifest(&'static str);
impl HlsManifestTransport for Manifest {
    fn read(&self, _: &url::Url, _: &XRequestContext) -> Result<Vec<u8>, AnalysisErrorCode> {
        Ok(self.0.as_bytes().to_vec())
    }
}
struct Bytes {
    calls: Mutex<Vec<(String, Option<HlsByteRange>)>>,
    fail: bool,
}
impl HlsSegmentTransport for Bytes {
    fn fetch(
        &self,
        url: &url::Url,
        range: Option<HlsByteRange>,
        path: &Path,
        maximum: u64,
        context: &XRequestContext,
    ) -> Result<u64, HlsTransferError> {
        assert!(!context.cancellation.is_canceled());
        self.calls.lock().unwrap().push((url.path().into(), range));
        if self.fail {
            return Err(AnalysisErrorCode::NetworkFailed.into());
        }
        let data = b"abcdefghijklmnop";
        let data = if let Some(range) = range {
            &data[range.offset as usize..(range.offset + range.length) as usize]
        } else {
            data.as_slice()
        };
        assert!(data.len() as u64 <= maximum);
        fs::write(path, data).unwrap();
        Ok(data.len() as u64)
    }
}
fn context() -> XRequestContext {
    XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}
fn run(
    document: &'static str,
    mode: HlsReadMode,
    transport: &dyn HlsSegmentTransport,
    parent: &Path,
    ctx: &XRequestContext,
) -> Result<local_media_downloader_lib::infrastructure::x::hls_local::LocalHlsInput, HlsTransferError>
{
    let snapshot = build_hls_snapshot(
        &"https://video.twimg.com/test/leaf.m3u8".parse().unwrap(),
        &Manifest(document),
        ctx,
    )
    .unwrap();
    materialize_hls(
        HlsMaterializationRequest {
            snapshot: &snapshot,
            parent,
            mode,
            network: ctx.network.clone(),
            cancellation: ctx.cancellation.clone(),
            deadline: Some(ctx.deadline),
        },
        transport,
    )
}
#[test]
fn x_hls_localizes_maps_and_exact_ranges_without_applying_offsets_twice() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context();
    let transport = Bytes {
        calls: Mutex::new(vec![]),
        fail: false,
    };
    let input=run("#EXTM3U\n#EXT-X-MAP:URI=\"data.mp4\",BYTERANGE=\"4@0\"\n#EXTINF:2,\n#EXT-X-BYTERANGE:4@4\ndata.mp4\n#EXTINF:2,\n#EXT-X-BYTERANGE:4\ndata.mp4\n#EXT-X-DISCONTINUITY\n#EXT-X-MAP:URI=\"other.mp4\"\n#EXTINF:2,\nlast.m4s\n#EXT-X-ENDLIST\n",HlsReadMode::Complete,&transport,root.path(),&ctx).unwrap();
    input.revalidate(&ctx.cancellation).unwrap();
    let document = fs::read_to_string(input.input()).unwrap();
    assert!(!document.contains("https://") && !document.contains("BYTERANGE"));
    assert!(document.contains("#EXT-X-DISCONTINUITY"));
    let mut files = fs::read_dir(input.input().parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().unwrap() != "m3u8")
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(
        files
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect::<Vec<_>>(),
        vec![
            b"abcd".to_vec(),
            b"efgh".to_vec(),
            b"ijkl".to_vec(),
            b"abcdefghijklmnop".to_vec(),
            b"abcdefghijklmnop".to_vec()
        ]
    );
    let parent = input.input().parent().unwrap().to_path_buf();
    fs::write(&files[1], b"tamper").unwrap();
    assert!(input.revalidate(&ctx.cancellation).is_err());
    drop(input);
    assert!(!parent.exists());
}
#[test]
fn x_hls_probe_prefix_does_not_replace_complete_download_or_omit_later_segments() {
    const DOC:&str="#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:3,\none.m4s\n#EXTINF:3,\ntwo.m4s\n#EXTINF:3,\nthree.m4s\n#EXT-X-ENDLIST\n";
    let root = tempfile::tempdir().unwrap();
    let ctx = context();
    let transport = Bytes {
        calls: Mutex::new(vec![]),
        fail: false,
    };
    let prefix = run(DOC, HlsReadMode::Probe, &transport, root.path(), &ctx).unwrap();
    assert!(!prefix.is_complete());
    assert_eq!(transport.calls.lock().unwrap().len(), 2);
    drop(prefix);
    transport.calls.lock().unwrap().clear();
    let complete = run(DOC, HlsReadMode::Complete, &transport, root.path(), &ctx).unwrap();
    assert!(complete.is_complete());
    assert_eq!(transport.calls.lock().unwrap().len(), 4);
    assert_eq!(
        fs::read_to_string(complete.input())
            .unwrap()
            .matches("#EXTINF")
            .count(),
        3
    );
}
#[test]
fn x_hls_failure_cancel_and_malformed_implicit_range_leave_no_partial_input() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context();
    let transport = Bytes {
        calls: Mutex::new(vec![]),
        fail: true,
    };
    assert!(run(
        "#EXTM3U\n#EXTINF:3,\none.ts\n#EXT-X-ENDLIST\n",
        HlsReadMode::Complete,
        &transport,
        root.path(),
        &ctx
    )
    .is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let good = Bytes {
        calls: Mutex::new(vec![]),
        fail: false,
    };
    assert!(run(
        "#EXTM3U\n#EXTINF:3,\n#EXT-X-BYTERANGE:4\none.ts\n#EXT-X-ENDLIST\n",
        HlsReadMode::Complete,
        &good,
        root.path(),
        &ctx
    )
    .is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let snapshot = build_hls_snapshot(
        &"https://video.twimg.com/test/leaf.m3u8".parse().unwrap(),
        &Manifest("#EXTM3U\n#EXTINF:3,\none.ts\n#EXT-X-ENDLIST\n"),
        &ctx,
    )
    .unwrap();
    ctx.cancellation.cancel();
    assert!(materialize_hls(
        HlsMaterializationRequest {
            snapshot: &snapshot,
            parent: root.path(),
            mode: HlsReadMode::Complete,
            network: ctx.network,
            cancellation: ctx.cancellation,
            deadline: Some(ctx.deadline)
        },
        &good
    )
    .is_err());
    assert!(good.calls.lock().unwrap().is_empty());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn x_hls_partial_storage_truncation_and_mid_transfer_cancel_clean_owned_files() {
    struct Fault(u8);
    impl HlsSegmentTransport for Fault {
        fn fetch(
            &self,
            _: &url::Url,
            _: Option<HlsByteRange>,
            path: &Path,
            _: u64,
            context: &XRequestContext,
        ) -> Result<u64, HlsTransferError> {
            fs::write(path, b"partial").unwrap();
            match self.0 {
                0 => Err(HlsTransferError::Io(std::io::ErrorKind::StorageFull)),
                1 => Ok(99), // The claimed completed length does not match disk.
                _ => {
                    context.cancellation.cancel();
                    Ok(7)
                }
            }
        }
    }
    for fault in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let ctx = context();
        let result = run(
            "#EXTM3U\n#EXTINF:3,\none.m4s\n#EXT-X-ENDLIST\n",
            HlsReadMode::Complete,
            &Fault(fault),
            root.path(),
            &ctx,
        );
        assert!(result.is_err());
        if fault == 0 {
            assert!(matches!(
                result.err(),
                Some(HlsTransferError::Io(std::io::ErrorKind::StorageFull))
            ));
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn x_hls_duplicate_assets_download_once_but_keep_each_playlist_position() {
    let root = tempfile::tempdir().unwrap();
    let transport = Bytes {
        calls: Mutex::new(vec![]),
        fail: false,
    };
    let input = run(
        "#EXTM3U\n#EXTINF:3,\none.m4s\n#EXTINF:3,\none.m4s\n#EXT-X-ENDLIST\n",
        HlsReadMode::Complete,
        &transport,
        root.path(),
        &context(),
    )
    .unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
    assert_eq!(
        fs::read_to_string(input.input())
            .unwrap()
            .matches("file:")
            .count(),
        2
    );
}

#[test]
fn x_hls_probe_samples_only_mapped_fmp4_and_keeps_complete_ranges_exact() {
    struct Prefix;
    impl HlsSegmentTransport for Prefix {
        fn fetch(
            &self,
            _: &url::Url,
            _: Option<HlsByteRange>,
            path: &Path,
            _: u64,
            _: &XRequestContext,
        ) -> Result<u64, HlsTransferError> {
            fs::write(path, b"abcdefghijklmnop").unwrap();
            Ok(16)
        }
        fn fetch_probe(
            &self,
            _: &url::Url,
            _: Option<HlsByteRange>,
            path: &Path,
            _: u64,
            _: &XRequestContext,
        ) -> Result<u64, HlsTransferError> {
            fs::write(path, b"abcd").unwrap();
            Ok(4)
        }
    }
    let root = tempfile::tempdir().unwrap();
    for (doc, mode, want) in [
        ("#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:3,\n#EXT-X-BYTERANGE:16@8\none.m4s\n#EXT-X-ENDLIST\n", HlsReadMode::Probe, vec![16, 4]),
        ("#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:3,\n#EXT-X-BYTERANGE:16@8\none.m4s\n#EXT-X-ENDLIST\n", HlsReadMode::Complete, vec![16, 16]),
        ("#EXTM3U\n#EXTINF:3,\none.mp4\n#EXT-X-ENDLIST\n", HlsReadMode::Probe, vec![16]),
        ("#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:3,\none.ts\n#EXT-X-ENDLIST\n", HlsReadMode::Probe, vec![16, 16]),
        ("#EXTM3U\n#EXT-X-MAP:URI=\"same.mp4\"\n#EXTINF:3,\nsame.mp4\n#EXT-X-ENDLIST\n", HlsReadMode::Probe, vec![16]),
        ("#EXTM3U\n#EXTINF:1,\none.m4s\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2,\none.m4s\n#EXT-X-ENDLIST\n", HlsReadMode::Probe, vec![16,16]),
    ] {
        let input = run(doc, mode, &Prefix, root.path(), &context()).unwrap();
        input.revalidate(&CancellationToken::new()).unwrap();
        let mut files = fs::read_dir(input.input().parent().unwrap()).unwrap().map(|e|e.unwrap().path()).filter(|p|p.extension().unwrap()!="m3u8").collect::<Vec<_>>();
        files.sort();
        assert_eq!(files.iter().map(|p| fs::metadata(p).unwrap().len()).collect::<Vec<_>>(), want);
    }
}
