use crate::domain::media::AnalysisErrorCode;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::Once,
};
const PREFIX: &str = "chendownloader-tiktok-";
const MARKER: &str = ".owner.json";
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema: u8,
    component: String,
    directory: String,
}
pub(crate) fn recover_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        recover(&std::env::temp_dir());
    });
}
pub(crate) struct OwnedTemporary {
    _lock: fs::File,
    directory: tempfile::TempDir,
}
impl OwnedTemporary {
    pub(crate) fn new() -> Result<Self, AnalysisErrorCode> {
        recover_once();
        let directory = tempfile::Builder::new()
            .prefix(PREFIX)
            .tempdir()
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join(MARKER))
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        file.lock().map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        let owner = Owner {
            schema: 1,
            component: "chendownloader-tiktok-core".into(),
            directory: directory
                .path()
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or(AnalysisErrorCode::ProcessFailed)?
                .into(),
        };
        file.write_all(&serde_json::to_vec(&owner).map_err(|_| AnalysisErrorCode::ProcessFailed)?)
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        file.sync_all()
            .map_err(|_| AnalysisErrorCode::ProcessFailed)?;
        Ok(Self {
            _lock: file,
            directory,
        })
    }
    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }
}
fn ordinary(path: &Path, directory: bool) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !metadata.file_type().is_symlink()
        && if directory {
            metadata.is_dir()
        } else {
            metadata.is_file() && metadata.len() <= 1024
        }
}
fn recover(root: &Path) {
    let Ok(parent) = root.canonicalize() else {
        return;
    };
    let Ok(entries) = fs::read_dir(&parent) else {
        return;
    };
    let mut reclaimed = 0;
    for entry in entries.take(4096).flatten() {
        if reclaimed >= 32 {
            break;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(PREFIX) || name.len() > 160 {
            continue;
        }
        let path = entry.path();
        if !ordinary(&path, true) || path.canonicalize().ok().as_deref() != Some(path.as_path()) {
            continue;
        }
        let marker = path.join(MARKER);
        if !ordinary(&marker, false) {
            continue;
        }
        let Ok(mut lock) = fs::OpenOptions::new().read(true).write(true).open(&marker) else {
            continue;
        };
        if lock.try_lock().is_err() {
            continue;
        }
        let mut bytes = Vec::new();
        if Read::by_ref(&mut lock)
            .take(1025)
            .read_to_end(&mut bytes)
            .is_err()
        {
            continue;
        }
        let Ok(owner) = serde_json::from_slice::<Owner>(&bytes) else {
            continue;
        };
        if owner.schema != 1
            || owner.component != "chendownloader-tiktok-core"
            || owner.directory != name
        {
            continue;
        }
        let Ok(contents) = fs::read_dir(&path) else {
            continue;
        };
        let contents = contents.take(4).collect::<Vec<_>>();
        if contents.len() > 2
            || contents.into_iter().any(|entry| match entry {
                Err(_) => true,
                Ok(e) => {
                    e.file_name() != MARKER
                        && !(e
                            .file_name()
                            .to_str()
                            .is_some_and(|n| n.starts_with("_MEI"))
                            && ordinary(&e.path(), true))
                }
            })
        {
            continue;
        }
        // The exclusive ownership lock proves no live native instance is using this root.
        if fs::remove_dir_all(&path).is_ok() {
            reclaimed += 1;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    fn marked(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        fs::write(
            path.join(MARKER),
            serde_json::to_vec(&Owner {
                schema: 1,
                component: "chendownloader-tiktok-core".into(),
                directory: name.into(),
            })
            .unwrap(),
        )
        .unwrap();
        fs::create_dir(path.join("_MEI123")).unwrap();
        path
    }
    #[test]
    fn recovery_skips_live_unmarked_and_non_owned_content_but_reclaims_stale() {
        let root = tempfile::tempdir().unwrap();
        let stale = marked(root.path(), "chendownloader-tiktok-stale");
        let live = marked(root.path(), "chendownloader-tiktok-live");
        let unrelated = marked(root.path(), "chendownloader-tiktok-unrelated");
        fs::write(unrelated.join("keep.txt"), b"keep").unwrap();
        let unknown = root.path().join("chendownloader-tiktok-unknown");
        fs::create_dir(&unknown).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(live.join(MARKER))
            .unwrap();
        lock.lock().unwrap();
        recover(root.path());
        assert!(!stale.exists());
        assert!(live.exists());
        assert!(unrelated.join("keep.txt").exists());
        assert!(unknown.exists());
        drop(lock);
        recover(root.path());
        assert!(!live.exists());
    }
}
