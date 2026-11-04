use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use crate::domain::download::{valid_opaque_id, DestinationApproval, DownloadErrorCode};

pub const DESTINATION_LIFETIME: Duration = Duration::from_secs(15 * 60);

pub trait DestinationValidator: Send + Sync {
    fn validate(&self, path: &Path) -> Result<PathBuf, DownloadErrorCode>;

    fn display_name(&self, path: &Path) -> String {
        sanitize_display_name(path.file_name().unwrap_or_else(|| OsStr::new("")))
    }
}

struct DestinationSession {
    destination_id: String,
    path: PathBuf,
    created_at: Instant,
}

pub struct ApprovedDestination {
    path: PathBuf,
}

impl ApprovedDestination {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

pub struct DestinationRegistry<V> {
    validator: V,
    next_id: AtomicU64,
    session: Mutex<Option<DestinationSession>>,
}

impl<V: DestinationValidator> DestinationRegistry<V> {
    pub fn new(validator: V) -> Self {
        Self {
            validator,
            next_id: AtomicU64::new(1),
            session: Mutex::new(None),
        }
    }

    pub fn approve(
        &self,
        path: PathBuf,
        now: Instant,
    ) -> Result<DestinationApproval, DownloadErrorCode> {
        let path = self.validator.validate(&path)?;
        let display_name = self.validator.display_name(&path);
        let mut session = self
            .session
            .lock()
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        if let Some(current) = session.as_ref().filter(|current| {
            current.path == path
                && now.saturating_duration_since(current.created_at) < DESTINATION_LIFETIME
        }) {
            // Workspaces share one saved folder. Revalidating it must not revoke
            // another platform's token, or extend that token's original lifetime.
            return DestinationApproval::new(current.destination_id.clone(), display_name)
                .map_err(|_| DownloadErrorCode::DestinationUnavailable);
        }
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| DownloadErrorCode::Unknown)?
            .to_string();
        let approval = DestinationApproval::new(id.clone(), display_name)
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        *session = Some(DestinationSession {
            destination_id: id,
            path,
            created_at: now,
        });
        Ok(approval)
    }

    pub fn resolve(
        &self,
        destination_id: &str,
        now: Instant,
    ) -> Result<ApprovedDestination, DownloadErrorCode> {
        if !valid_opaque_id(destination_id) {
            return Err(DownloadErrorCode::DestinationUnavailable);
        }
        let mut session = self
            .session
            .lock()
            .map_err(|_| DownloadErrorCode::DestinationUnavailable)?;
        let current = session
            .as_ref()
            .filter(|current| current.destination_id == destination_id)
            .ok_or(DownloadErrorCode::DestinationUnavailable)?;
        if now.saturating_duration_since(current.created_at) >= DESTINATION_LIFETIME {
            *session = None;
            return Err(DownloadErrorCode::DestinationUnavailable);
        }
        let path = current.path.clone();
        drop(session);
        Ok(ApprovedDestination {
            path: self.validator.validate(&path)?,
        })
    }

    pub fn clear(&self) {
        if let Ok(mut session) = self.session.lock() {
            *session = None;
        }
    }
}

pub fn approve_selected_destination<V: DestinationValidator>(
    registry: &DestinationRegistry<V>,
    selected: Option<PathBuf>,
    now: Instant,
) -> Result<Option<DestinationApproval>, DownloadErrorCode> {
    selected.map(|path| registry.approve(path, now)).transpose()
}

pub fn sanitize_display_name(value: &OsStr) -> String {
    let sanitized: String = value
        .to_string_lossy()
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    *character,
                    '\u{202a}'
                        ..='\u{202e}' | '\u{2066}'
                        ..='\u{2069}' | '\u{200e}' | '\u{200f}'
                )
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(80)
        .collect();
    if sanitized.is_empty() {
        "Selected folder".to_owned()
    } else {
        sanitized
    }
}
