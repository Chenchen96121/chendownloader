//! Ephemeral evidence from the pinned extractor's exact response bytes.
use crate::{
    application::{
        bilibili_source::{BilibiliPageEvidence, BilibiliRequestContext},
        process::{CancellationToken, ProcessExit},
    },
    domain::{bilibili::BilibiliVideoRequest, media::AnalysisErrorCode},
    infrastructure::bilibili::page::parse_bilibili_page,
};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

const FILE_LIMIT: u64 = 4 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 16 * 1024 * 1024;
const COUNT_LIMIT: usize = 16;

fn inventory(directory: &Path) -> Result<Vec<PathBuf>, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let mut paths = Vec::new();
    let mut total = 0_u64;
    for entry in fs::read_dir(directory).map_err(|_| invalid)? {
        let path = entry.map_err(|_| invalid)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| invalid)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || reparse(&metadata)
            || path.extension().and_then(|s| s.to_str()) != Some("dump")
            || metadata.len() > FILE_LIMIT
            || paths.len() == COUNT_LIMIT
        {
            return Err(invalid);
        }
        total = total.checked_add(metadata.len()).ok_or(invalid)?;
        if total > TOTAL_LIMIT {
            return Err(invalid);
        }
        paths.push(path);
    }
    Ok(paths)
}

// The trusted tool writes before returning the page. Watch the private cwd
// during its bounded run as well as afterwards; no page/diagnostic survives it.
pub(super) fn run_with_budget(
    directory: &Path,
    context: &BilibiliRequestContext,
    run: impl FnOnce(CancellationToken) -> Result<ProcessExit, AnalysisErrorCode>,
) -> Result<ProcessExit, AnalysisErrorCode> {
    let exceeded = AtomicBool::new(false);
    let process_cancellation = CancellationToken::new();
    if context.cancellation().is_canceled() {
        process_cancellation.cancel();
    }
    std::thread::scope(|scope| {
        let (stop, receiver) = mpsc::channel();
        let exceeded_ref = &exceeded;
        let process_cancel = process_cancellation.clone();
        let watcher = scope.spawn(move || {
            while receiver.recv_timeout(Duration::from_millis(50))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                if context.cancellation().is_canceled() {
                    process_cancel.cancel();
                    break;
                }
                if inventory(directory).is_err() {
                    exceeded_ref.store(true, Ordering::Release);
                    process_cancel.cancel();
                    break;
                }
            }
        });
        let result = run(process_cancellation);
        let _ = stop.send(());
        watcher
            .join()
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        if exceeded.load(Ordering::Acquire) {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        inventory(directory)?;
        result
    })
}

pub(super) fn verify_consumed_page(
    directory: &Path,
    request: &BilibiliVideoRequest,
    expected: &BilibiliPageEvidence,
) -> Result<(), AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let mut found = false;
    for path in inventory(directory)? {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(|_| invalid)?
            .take(FILE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid)?;
        if bytes.len() as u64 > FILE_LIMIT {
            return Err(invalid);
        }
        if !bytes
            .windows(b"window.__INITIAL_STATE__".len())
            .any(|s| s == b"window.__INITIAL_STATE__")
        {
            continue;
        }
        if found {
            return Err(invalid);
        }
        let consumed = parse_bilibili_page(request, &bytes)?;
        if consumed.identity() != expected.identity() {
            return Err(AnalysisErrorCode::ContentChanged);
        }
        found = true;
    }
    if found {
        Ok(())
    } else {
        Err(invalid)
    }
}

#[cfg(windows)]
fn reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}
#[cfg(not(windows))]
fn reparse(_: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{network_profile::NetworkProfile, process::CancellationToken};
    use std::{sync::Arc, time::Instant};
    #[test]
    fn dump_budget_violation_does_not_fabricate_a_user_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let context = BilibiliRequestContext::new(
            CancellationToken::new(),
            Instant::now() + Duration::from_secs(2),
            Arc::new(NetworkProfile::current()),
        );
        let result = run_with_budget(directory.path(), &context, |process_cancel| {
            fs::File::create(directory.path().join("too-big.dump"))
                .unwrap()
                .set_len(FILE_LIMIT + 1)
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            while !process_cancel.is_canceled() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(AnalysisErrorCode::ProcessFailed)
        });
        assert_eq!(result, Err(AnalysisErrorCode::InvalidMetadata));
        assert!(!context.cancellation().is_canceled());
    }

    #[test]
    fn caller_cancellation_still_reaches_the_child_process_token() {
        let directory = tempfile::tempdir().unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let context = BilibiliRequestContext::new(
            cancellation,
            Instant::now() + Duration::from_secs(2),
            Arc::new(NetworkProfile::current()),
        );
        let result = run_with_budget(directory.path(), &context, |process_cancel| {
            assert!(process_cancel.is_canceled());
            Err(AnalysisErrorCode::ProcessFailed)
        });
        assert_eq!(result, Err(AnalysisErrorCode::ProcessFailed));
    }
}
