use super::process::CancellationToken;
use crate::domain::{
    download::{DownloadErrorCode, DownloadProgress, DownloadState},
    instagram::{InstagramDownloadProgress, InstagramJobState},
    media::Platform,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadOwner {
    Youtube,
    Instagram,
    X,
    Bilibili,
    Tiktok,
    ToolRepair,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Queued,
    Preparing,
    Downloading,
    Processing,
    Verifying,
    Publishing,
    Completed,
    Partial,
    Failed,
    Canceled,
}
impl TaskPhase {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Partial | Self::Failed | Self::Canceled
        )
    }
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub job_id: String,
    pub platform: Platform,
    pub title: String,
    pub state: TaskPhase,
    pub percent: Option<f64>,
    pub error_code: Option<DownloadErrorCode>,
    pub display_name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadQueueSnapshot {
    pub limit: u8,
    pub tasks: Vec<DownloadTask>,
}

struct ActiveDownload {
    token: u64,
    owner: DownloadOwner,
}
struct GateState {
    next_token: u64,
    exclusive: Option<ActiveDownload>,
    waiting: VecDeque<u64>,
    running: Vec<u64>,
    tasks: BTreeMap<u64, DownloadTask>,
    limit: u8,
    closed: bool,
    cancellations: BTreeMap<u64, CancellationToken>,
}
struct GateInner {
    state: Mutex<GateState>,
    changed: Condvar,
}
#[derive(Clone)]
pub struct DownloadActivityGate {
    inner: Arc<GateInner>,
}

impl DownloadActivityGate {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(GateInner {
                state: Mutex::new(GateState {
                    next_token: 1,
                    exclusive: None,
                    waiting: VecDeque::new(),
                    running: vec![],
                    tasks: BTreeMap::new(),
                    cancellations: BTreeMap::new(),
                    limit: 2,
                    closed: false,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    /// Exclusive operation, notably offline tool repair. Pending tasks count too.
    pub fn try_acquire(&self, owner: DownloadOwner) -> Result<DownloadLease, DownloadErrorCode> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        if state.closed
            || state.exclusive.is_some()
            || !state.waiting.is_empty()
            || !state.running.is_empty()
        {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        let token = next_token(&mut state)?;
        state.exclusive = Some(ActiveDownload { token, owner });
        Ok(DownloadLease {
            inner: Arc::clone(&self.inner),
            token,
            exclusive: true,
        })
    }

    pub fn reserve(
        &self,
        owner: DownloadOwner,
        title: &str,
    ) -> Result<DownloadLease, DownloadErrorCode> {
        let platform = match owner {
            DownloadOwner::Youtube => Platform::Youtube,
            DownloadOwner::Instagram => Platform::Instagram,
            DownloadOwner::X => Platform::X,
            DownloadOwner::Bilibili => Platform::Bilibili,
            DownloadOwner::Tiktok => Platform::Tiktok,
            DownloadOwner::ToolRepair => return Err(DownloadErrorCode::InvalidRequest),
        };
        if title.trim().is_empty()
            || title.chars().count() > 512
            || title.chars().any(char::is_control)
        {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        if state.closed
            || state.exclusive.is_some()
            || state.waiting.len() + state.running.len() >= 32
        {
            return Err(DownloadErrorCode::DownloadInProgress);
        }
        let token = next_token(&mut state)?;
        state.waiting.push_back(token);
        state.tasks.insert(
            token,
            DownloadTask {
                job_id: token.to_string(),
                platform,
                title: title.to_owned(),
                state: TaskPhase::Queued,
                percent: None,
                error_code: None,
                display_name: None,
            },
        );
        prune(&mut state);
        self.inner.changed.notify_all();
        Ok(DownloadLease {
            inner: Arc::clone(&self.inner),
            token,
            exclusive: false,
        })
    }

    pub fn set_limit(&self, limit: u8) -> Result<(), DownloadErrorCode> {
        if !(1..=4).contains(&limit) {
            return Err(DownloadErrorCode::InvalidRequest);
        }
        self.inner
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?
            .limit = limit;
        self.inner.changed.notify_all();
        Ok(())
    }
    pub fn snapshot(&self) -> Result<DownloadQueueSnapshot, DownloadErrorCode> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        Ok(DownloadQueueSnapshot {
            limit: state.limit,
            tasks: state.tasks.values().cloned().collect(),
        })
    }
    pub fn active_owner(&self) -> Option<DownloadOwner> {
        self.inner.state.lock().ok().and_then(|state| {
            state.exclusive.as_ref().map(|a| a.owner).or_else(|| {
                state
                    .running
                    .first()
                    .or_else(|| state.waiting.front())
                    .and_then(|id| state.tasks.get(id))
                    .map(|task| match task.platform {
                        Platform::Youtube => DownloadOwner::Youtube,
                        Platform::Instagram => DownloadOwner::Instagram,
                        Platform::X => DownloadOwner::X,
                        Platform::Bilibili => DownloadOwner::Bilibili,
                        Platform::Tiktok => DownloadOwner::Tiktok,
                    })
            })
        })
    }
    pub fn close(&self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.closed = true;
            for cancellation in state.cancellations.values() {
                cancellation.cancel();
            }
            self.inner.changed.notify_all();
        }
    }

    pub fn record_youtube(&self, progress: &DownloadProgress) {
        self.record_single_video(DownloadOwner::Youtube, progress);
    }
    pub fn record_single_video(&self, owner: DownloadOwner, progress: &DownloadProgress) {
        let platform = match owner {
            DownloadOwner::Youtube => Platform::Youtube,
            DownloadOwner::Bilibili => Platform::Bilibili,
            _ => return,
        };
        let same_owner = self
            .inner
            .state
            .lock()
            .ok()
            .and_then(|state| {
                progress
                    .job_id
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| state.tasks.get(&id).map(|task| task.platform == platform))
            })
            .unwrap_or(false);
        if !same_owner {
            return;
        }
        let phase = match progress.state {
            DownloadState::Preparing => TaskPhase::Preparing,
            DownloadState::Downloading => TaskPhase::Downloading,
            DownloadState::Processing => TaskPhase::Processing,
            DownloadState::Verifying => TaskPhase::Verifying,
            DownloadState::Completed => TaskPhase::Completed,
            DownloadState::Failed => TaskPhase::Failed,
            DownloadState::Canceled => TaskPhase::Canceled,
        };
        self.record(&progress.job_id, phase, progress.percent);
    }
    pub fn record_instagram(&self, progress: &InstagramDownloadProgress) {
        let is_instagram = self
            .inner
            .state
            .lock()
            .ok()
            .and_then(|state| {
                progress.job_id.parse::<u64>().ok().and_then(|id| {
                    state
                        .tasks
                        .get(&id)
                        .map(|task| task.platform == Platform::Instagram)
                })
            })
            .unwrap_or(false);
        if !is_instagram {
            return;
        }
        let phase = match progress.state {
            InstagramJobState::Queued => TaskPhase::Queued,
            InstagramJobState::Downloading => TaskPhase::Downloading,
            InstagramJobState::Processing => TaskPhase::Processing,
            InstagramJobState::Verifying => TaskPhase::Verifying,
            InstagramJobState::Publishing => TaskPhase::Publishing,
            InstagramJobState::Completed => TaskPhase::Completed,
            InstagramJobState::Partial => TaskPhase::Partial,
            InstagramJobState::Failed => TaskPhase::Failed,
            InstagramJobState::Canceled => TaskPhase::Canceled,
        };
        let percent = if phase == TaskPhase::Queued {
            None
        } else {
            Some(
                (f64::from(progress.item_index.saturating_sub(1))
                    + progress.percent.unwrap_or(0.0) / 100.0)
                    / f64::from(progress.item_count)
                    * 100.0,
            )
        };
        self.record(&progress.job_id, phase, percent);
    }
    pub fn record_x(&self, progress: &crate::domain::x::XDownloadProgress) {
        use crate::domain::x::XJobState;
        if progress.validate().is_err() {
            return;
        }
        let is_x = self
            .inner
            .state
            .lock()
            .ok()
            .and_then(|s| {
                progress
                    .job_id
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| s.tasks.get(&id).map(|t| t.platform == Platform::X))
            })
            .unwrap_or(false);
        if !is_x {
            return;
        }
        let phase = match progress.state {
            XJobState::Queued => TaskPhase::Queued,
            XJobState::Downloading => TaskPhase::Downloading,
            XJobState::Processing => TaskPhase::Processing,
            XJobState::Verifying => TaskPhase::Verifying,
            XJobState::Publishing => TaskPhase::Publishing,
            XJobState::Completed => TaskPhase::Completed,
            XJobState::Partial => TaskPhase::Partial,
            XJobState::Failed => TaskPhase::Failed,
            XJobState::Canceled => TaskPhase::Canceled,
        };
        self.record(
            &progress.job_id,
            phase,
            if progress.item_count == 1 {
                progress.percent
            } else {
                None
            },
        );
    }
    pub fn record_tiktok(&self, progress: &crate::domain::tiktok::TikTokDownloadProgress) {
        use crate::domain::tiktok::TikTokJobState;
        if progress.validate().is_err() {
            return;
        }
        let is_x = self
            .inner
            .state
            .lock()
            .ok()
            .and_then(|s| {
                progress
                    .job_id
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| s.tasks.get(&id).map(|t| t.platform == Platform::Tiktok))
            })
            .unwrap_or(false);
        if !is_x {
            return;
        }
        let phase = match progress.state {
            TikTokJobState::Queued => TaskPhase::Queued,
            TikTokJobState::Downloading => TaskPhase::Downloading,
            TikTokJobState::Processing => TaskPhase::Processing,
            TikTokJobState::Verifying => TaskPhase::Verifying,
            TikTokJobState::Publishing => TaskPhase::Publishing,
            TikTokJobState::Completed => TaskPhase::Completed,
            TikTokJobState::Partial => TaskPhase::Partial,
            TikTokJobState::Failed => TaskPhase::Failed,
            TikTokJobState::Canceled => TaskPhase::Canceled,
        };
        self.record(
            &progress.job_id,
            phase,
            if progress.item_count == 1 {
                progress.percent
            } else {
                None
            },
        );
    }
    fn record(&self, job_id: &str, phase: TaskPhase, percent: Option<f64>) {
        if let Ok(mut state) = self.inner.state.lock() {
            let Ok(token) = job_id.parse() else {
                return;
            };
            let pending = state.waiting.contains(&token);
            if let Some(task) = state.tasks.get_mut(&token) {
                if task.state.terminal() || (pending && !phase.terminal()) {
                    return;
                }
                task.state = phase;
                if phase == TaskPhase::Completed {
                    task.percent = Some(100.0);
                } else if let Some(percent) =
                    percent.filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
                {
                    task.percent = Some(task.percent.unwrap_or(0.0).max(percent));
                }
            }
        }
    }
    pub fn finish_details(
        &self,
        job_id: &str,
        error: Option<DownloadErrorCode>,
        name: Option<String>,
    ) {
        if let Ok(mut state) = self.inner.state.lock() {
            if let Some(task) = job_id
                .parse::<u64>()
                .ok()
                .and_then(|id| state.tasks.get_mut(&id))
            {
                task.error_code = error;
                task.display_name = name;
            }
        }
    }
}
impl Default for DownloadActivityGate {
    fn default() -> Self {
        Self::new()
    }
}
fn next_token(state: &mut GateState) -> Result<u64, DownloadErrorCode> {
    let token = state.next_token;
    state.next_token = token
        .checked_add(1)
        .filter(|n| *n <= (9_007_199_254_740_991 - 32) / 100)
        .ok_or(DownloadErrorCode::Unknown)?;
    Ok(token)
}
fn prune(state: &mut GateState) {
    let terminal: Vec<_> = state
        .tasks
        .iter()
        .filter(|(_, t)| t.state.terminal())
        .map(|(id, _)| *id)
        .collect();
    let excess = terminal.len().saturating_sub(100);
    for id in terminal.into_iter().take(excess) {
        state.tasks.remove(&id);
    }
}

pub struct DownloadLease {
    inner: Arc<GateInner>,
    token: u64,
    exclusive: bool,
}
impl DownloadLease {
    pub fn job_id(&self) -> String {
        self.token.to_string()
    }
    pub fn wait_turn(&self, cancellation: &CancellationToken) -> Result<(), DownloadErrorCode> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DownloadErrorCode::Unknown)?;
        state.cancellations.insert(self.token, cancellation.clone());
        loop {
            if state.closed {
                cancellation.cancel();
            }
            if cancellation.is_canceled() {
                return Err(DownloadErrorCode::ProcessFailed);
            }
            if self.exclusive || state.running.contains(&self.token) {
                return Ok(());
            }
            if state.waiting.front() == Some(&self.token)
                && state.running.len() < usize::from(state.limit)
            {
                state.waiting.pop_front();
                state.running.push(self.token);
                if let Some(task) = state.tasks.get_mut(&self.token) {
                    task.state = TaskPhase::Preparing;
                }
                self.inner.changed.notify_all();
                return Ok(());
            }
            let (next, _) = self
                .inner
                .changed
                .wait_timeout(state, Duration::from_millis(50))
                .map_err(|_| DownloadErrorCode::Unknown)?;
            state = next;
        }
    }
}
impl Drop for DownloadLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inner.state.lock() {
            if self.exclusive {
                if state
                    .exclusive
                    .as_ref()
                    .is_some_and(|a| a.token == self.token)
                {
                    state.exclusive = None;
                }
            } else {
                state.waiting.retain(|id| *id != self.token);
                state.running.retain(|id| *id != self.token);
                state.cancellations.remove(&self.token);
                if let Some(task) = state.tasks.get_mut(&self.token) {
                    if !task.state.terminal() {
                        task.state = TaskPhase::Failed;
                        task.error_code = Some(DownloadErrorCode::ProcessFailed);
                    }
                }
                prune(&mut state);
            }
            self.inner.changed.notify_all();
        }
    }
}
