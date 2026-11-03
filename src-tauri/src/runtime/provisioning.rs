use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    application::{
        download_activity::{DownloadActivityGate, DownloadLease, DownloadOwner},
        portable_package::TrustedToolPackageSource,
        tool_bootstrap::{ManagedToolStore, ToolVersionProbe},
        tool_provisioning::{OfflineToolProvisioner, ProvisioningMode, ToolProvisioner},
    },
    domain::{
        managed_tools::{ManagedToolError, ManagedToolErrorCode},
        tool_provisioning::{
            ToolProvisioningErrorCode, ToolProvisioningState, ToolProvisioningStatus,
        },
    },
};

struct RuntimeState {
    status: ToolProvisioningStatus,
    running: bool,
    last_repair_error: Option<ToolProvisioningErrorCode>,
}

struct SharedState {
    state: Mutex<RuntimeState>,
    idle: Condvar,
}

pub struct ToolProvisioningRuntime {
    provisioner: Arc<dyn ToolProvisioner + Send + Sync>,
    activity: Arc<DownloadActivityGate>,
    shared: Arc<SharedState>,
    worker: Mutex<Option<JoinHandle<()>>>,
    repair_request: Mutex<()>,
}

impl ToolProvisioningRuntime {
    pub fn new(
        provisioner: Arc<dyn ToolProvisioner + Send + Sync>,
        activity: Arc<DownloadActivityGate>,
    ) -> Self {
        Self {
            provisioner,
            activity,
            shared: Arc::new(SharedState {
                state: Mutex::new(RuntimeState {
                    status: ToolProvisioningStatus::preparing(),
                    running: false,
                    last_repair_error: None,
                }),
                idle: Condvar::new(),
            }),
            worker: Mutex::new(None),
            repair_request: Mutex::new(()),
        }
    }

    pub fn start(&self) -> Result<ToolProvisioningStatus, ToolProvisioningErrorCode> {
        self.begin(ProvisioningMode::Startup, None)
    }

    pub fn status(&self) -> ToolProvisioningStatus {
        self.shared
            .state
            .lock()
            .map(|state| state.status.clone())
            .unwrap_or_else(|_| {
                ToolProvisioningStatus::failed(ToolProvisioningErrorCode::ActivationFailed)
            })
    }

    pub fn request_repair(&self) -> Result<ToolProvisioningStatus, ToolProvisioningErrorCode> {
        let _request = self
            .repair_request
            .try_lock()
            .map_err(|_| ToolProvisioningErrorCode::Busy)?;
        let activity = self
            .activity
            .try_acquire(DownloadOwner::ToolRepair)
            .map_err(|_| ToolProvisioningErrorCode::DownloadInProgress)?;
        self.begin(ProvisioningMode::Repair, Some(activity))?;
        self.wait_until_idle();
        self.join_finished_worker();
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| ToolProvisioningErrorCode::ActivationFailed)?;
        if let Some(error) = state.last_repair_error.take() {
            return Err(error);
        }
        Ok(state.status.clone())
    }

    pub fn is_ready(&self) -> bool {
        self.status().state() == ToolProvisioningState::Ready
    }

    pub fn shutdown(&self) {
        self.wait_until_idle();
        self.join_finished_worker();
    }

    #[doc(hidden)]
    pub fn wait_for_idle(&self, timeout: Duration) -> bool {
        let Ok(state) = self.shared.state.lock() else {
            return false;
        };
        let Ok((state, wait)) = self
            .shared
            .idle
            .wait_timeout_while(state, timeout, |state| state.running)
        else {
            return false;
        };
        !wait.timed_out() && !state.running
    }

    fn begin(
        &self,
        mode: ProvisioningMode,
        activity: Option<DownloadLease>,
    ) -> Result<ToolProvisioningStatus, ToolProvisioningErrorCode> {
        {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| ToolProvisioningErrorCode::ActivationFailed)?;
            if state.running {
                return Err(ToolProvisioningErrorCode::Busy);
            }
        }
        self.join_finished_worker();
        let next = match mode {
            ProvisioningMode::Startup => ToolProvisioningStatus::preparing(),
            ProvisioningMode::Repair => ToolProvisioningStatus::repairing(),
        };
        {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| ToolProvisioningErrorCode::ActivationFailed)?;
            if state.running {
                return Err(ToolProvisioningErrorCode::Busy);
            }
            state.running = true;
            state.last_repair_error = None;
            state.status = next.clone();
        }

        let provisioner = Arc::clone(&self.provisioner);
        let shared = Arc::clone(&self.shared);
        let spawn = thread::Builder::new()
            .name(
                match mode {
                    ProvisioningMode::Startup => "tool-provision-startup",
                    ProvisioningMode::Repair => "tool-provision-repair",
                }
                .to_owned(),
            )
            .spawn(move || run_worker(provisioner, shared, mode, activity));
        match spawn {
            Ok(worker) => {
                if let Ok(mut slot) = self.worker.lock() {
                    *slot = Some(worker);
                    Ok(next)
                } else {
                    let _ = worker.join();
                    self.fail_spawn();
                    Err(ToolProvisioningErrorCode::ActivationFailed)
                }
            }
            Err(_) => {
                self.fail_spawn();
                Err(ToolProvisioningErrorCode::ActivationFailed)
            }
        }
    }

    fn fail_spawn(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.running = false;
            state.status =
                ToolProvisioningStatus::failed(ToolProvisioningErrorCode::ActivationFailed);
            self.shared.idle.notify_all();
        }
    }

    fn wait_until_idle(&self) {
        if let Ok(state) = self.shared.state.lock() {
            drop(self.shared.idle.wait_while(state, |state| state.running));
        }
    }

    fn join_finished_worker(&self) {
        let worker = self.worker.lock().ok().and_then(|mut slot| slot.take());
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

pub struct OwnedOfflineToolProvisioner<S, M, P> {
    source: S,
    store: Arc<M>,
    probe: P,
    target: String,
}

impl<S, M, P> OwnedOfflineToolProvisioner<S, M, P> {
    pub fn new(source: S, store: Arc<M>, probe: P, target: &str) -> Self {
        Self {
            source,
            store,
            probe,
            target: target.to_owned(),
        }
    }
}

impl<S, M, P> ToolProvisioner for OwnedOfflineToolProvisioner<S, M, P>
where
    S: TrustedToolPackageSource,
    M: ManagedToolStore,
    P: ToolVersionProbe,
{
    fn provision(
        &self,
        mode: ProvisioningMode,
    ) -> Result<crate::domain::managed_tools::ToolInventory, ManagedToolError> {
        OfflineToolProvisioner::new(&self.source, self.store.as_ref(), &self.probe, &self.target)
            .provision(mode)
    }
}

fn run_worker(
    provisioner: Arc<dyn ToolProvisioner + Send + Sync>,
    shared: Arc<SharedState>,
    mode: ProvisioningMode,
    activity: Option<DownloadLease>,
) {
    let _activity = activity;
    let result = catch_unwind(AssertUnwindSafe(|| provisioner.provision(mode)))
        .unwrap_or_else(|_| Err(activation_panic()));
    let (status, repair_error) = match (mode, result) {
        (_, Ok(_)) => (ToolProvisioningStatus::ready(), None),
        (ProvisioningMode::Startup, Err(error)) => {
            (ToolProvisioningStatus::failed(map_error(&error)), None)
        }
        (ProvisioningMode::Repair, Err(error)) => {
            let public_error = map_error(&error);
            let fallback = catch_unwind(AssertUnwindSafe(|| {
                provisioner.provision(ProvisioningMode::Startup)
            }))
            .unwrap_or_else(|_| Err(activation_panic()));
            let status = match fallback {
                Ok(_) => ToolProvisioningStatus::ready(),
                Err(_) => ToolProvisioningStatus::failed(public_error),
            };
            (status, Some(public_error))
        }
    };
    if let Ok(mut state) = shared.state.lock() {
        state.status = status;
        state.last_repair_error = repair_error;
        state.running = false;
        shared.idle.notify_all();
    }
}

fn map_error(error: &ManagedToolError) -> ToolProvisioningErrorCode {
    match error.code {
        ManagedToolErrorCode::ToolMissing => ToolProvisioningErrorCode::PackageMissing,
        ManagedToolErrorCode::UnsupportedTarget
        | ManagedToolErrorCode::InvalidToolManifest
        | ManagedToolErrorCode::ToolIntegrityFailed => ToolProvisioningErrorCode::PackageInvalid,
        ManagedToolErrorCode::ToolProbeFailed | ManagedToolErrorCode::ToolVersionMismatch => {
            ToolProvisioningErrorCode::ToolProbeFailed
        }
        ManagedToolErrorCode::ToolActivationFailed => ToolProvisioningErrorCode::ActivationFailed,
    }
}

fn activation_panic() -> ManagedToolError {
    ManagedToolError::new(
        ManagedToolErrorCode::ToolActivationFailed,
        "provisioning worker stopped unexpectedly",
    )
}
