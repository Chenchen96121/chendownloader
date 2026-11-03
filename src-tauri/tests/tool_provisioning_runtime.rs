use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use local_media_downloader_lib::{
    application::{
        download_activity::{DownloadActivityGate, DownloadOwner},
        tool_provisioning::{ProvisioningMode, ToolProvisioner},
    },
    domain::{
        managed_tools::{
            ManagedToolError, ManagedToolErrorCode, ToolInventory, ToolName, ToolState, ToolStatus,
            BUILD_TARGET,
        },
        tool_provisioning::{
            ToolProvisioningErrorCode, ToolProvisioningState, ToolProvisioningStatus,
        },
    },
    runtime::provisioning::ToolProvisioningRuntime,
};

struct BlockingProvisioner {
    script: Mutex<VecDeque<Result<ToolInventory, ManagedToolError>>>,
    calls: Mutex<Vec<ProvisioningMode>>,
    entered: Condvar,
    permits: Mutex<usize>,
    released: Condvar,
}

impl BlockingProvisioner {
    fn new(script: Vec<Result<ToolInventory, ManagedToolError>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            calls: Mutex::new(Vec::new()),
            entered: Condvar::new(),
            permits: Mutex::new(0),
            released: Condvar::new(),
        })
    }

    fn wait_for_calls(&self, count: usize) {
        let calls = self.calls.lock().unwrap();
        let (calls, timeout) = self
            .entered
            .wait_timeout_while(calls, Duration::from_secs(3), |calls| calls.len() < count)
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "provisioning call {count} never started"
        );
        assert!(calls.len() >= count);
    }

    fn release(&self, count: usize) {
        *self.permits.lock().unwrap() += count;
        self.released.notify_all();
    }

    fn modes(&self) -> Vec<ProvisioningMode> {
        self.calls.lock().unwrap().clone()
    }
}

impl ToolProvisioner for BlockingProvisioner {
    fn provision(&self, mode: ProvisioningMode) -> Result<ToolInventory, ManagedToolError> {
        self.calls.lock().unwrap().push(mode);
        self.entered.notify_all();
        let permits = self.permits.lock().unwrap();
        let mut permits = self
            .released
            .wait_while(permits, |permits| *permits == 0)
            .unwrap();
        *permits -= 1;
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .expect("fixture result for every provisioning call")
    }
}

#[test]
fn state_contract_is_closed_and_rejects_impossible_combinations() {
    assert_eq!(
        serde_json::to_value(
            ToolProvisioningStatus::new(ToolProvisioningState::Ready, None).unwrap()
        )
        .unwrap(),
        serde_json::json!({ "state": "ready" })
    );
    assert_eq!(
        serde_json::to_value(
            ToolProvisioningStatus::new(
                ToolProvisioningState::Failed,
                Some(ToolProvisioningErrorCode::PackageInvalid),
            )
            .unwrap(),
        )
        .unwrap(),
        serde_json::json!({ "state": "failed", "errorCode": "package_invalid" })
    );
    assert_eq!(
        [
            ToolProvisioningState::Preparing,
            ToolProvisioningState::Ready,
            ToolProvisioningState::Repairing,
            ToolProvisioningState::Failed,
        ]
        .map(|state| serde_json::to_value(state).unwrap()),
        ["preparing", "ready", "repairing", "failed"].map(serde_json::Value::from)
    );
    assert_eq!(
        [
            ToolProvisioningErrorCode::PackageMissing,
            ToolProvisioningErrorCode::PackageInvalid,
            ToolProvisioningErrorCode::ToolProbeFailed,
            ToolProvisioningErrorCode::ActivationFailed,
            ToolProvisioningErrorCode::Busy,
            ToolProvisioningErrorCode::DownloadInProgress,
        ]
        .map(|code| serde_json::to_value(code).unwrap()),
        [
            "package_missing",
            "package_invalid",
            "tool_probe_failed",
            "activation_failed",
            "busy",
            "download_in_progress",
        ]
        .map(serde_json::Value::from)
    );
    assert!(ToolProvisioningStatus::new(
        ToolProvisioningState::Ready,
        Some(ToolProvisioningErrorCode::PackageMissing),
    )
    .is_err());
    assert!(ToolProvisioningStatus::new(ToolProvisioningState::Failed, None).is_err());
    assert!(ToolProvisioningStatus::new(
        ToolProvisioningState::Failed,
        Some(ToolProvisioningErrorCode::Busy),
    )
    .is_err());
}

#[test]
fn startup_transitions_from_preparing_to_ready_and_shutdown_joins() {
    let provisioner = BlockingProvisioner::new(vec![Ok(ready_inventory())]);
    let runtime = Arc::new(runtime(Arc::clone(&provisioner), activity()));

    assert_eq!(
        runtime.start().unwrap().state(),
        ToolProvisioningState::Preparing
    );
    provisioner.wait_for_calls(1);
    assert_eq!(runtime.status().state(), ToolProvisioningState::Preparing);
    assert!(!runtime.is_ready());

    let (done_tx, done_rx) = mpsc::channel();
    let joining = Arc::clone(&runtime);
    let joiner = thread::spawn(move || {
        joining.shutdown();
        done_tx.send(()).unwrap();
    });
    assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
    provisioner.release(1);
    done_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    joiner.join().unwrap();

    assert_eq!(runtime.status().state(), ToolProvisioningState::Ready);
    assert!(runtime.is_ready());
    assert_eq!(provisioner.modes(), [ProvisioningMode::Startup]);
}

#[test]
fn repair_transitions_to_ready_and_rejects_repeated_or_early_requests() {
    let provisioner = BlockingProvisioner::new(vec![Ok(ready_inventory()), Ok(ready_inventory())]);
    let runtime = Arc::new(runtime(Arc::clone(&provisioner), activity()));
    runtime.start().unwrap();
    provisioner.wait_for_calls(1);
    assert_eq!(
        runtime.request_repair().unwrap_err(),
        ToolProvisioningErrorCode::Busy
    );
    provisioner.release(1);
    assert!(runtime.wait_for_idle(Duration::from_secs(3)));

    let repairing = Arc::clone(&runtime);
    let repair = thread::spawn(move || repairing.request_repair());
    provisioner.wait_for_calls(2);
    assert_eq!(runtime.status().state(), ToolProvisioningState::Repairing);
    assert_eq!(
        runtime.request_repair().unwrap_err(),
        ToolProvisioningErrorCode::Busy
    );
    provisioner.release(1);

    assert_eq!(
        repair.join().unwrap().unwrap().state(),
        ToolProvisioningState::Ready
    );
    assert_eq!(
        provisioner.modes(),
        [ProvisioningMode::Startup, ProvisioningMode::Repair]
    );
}

#[test]
fn repair_is_rejected_while_a_download_is_active() {
    let activity = activity();
    let provisioner = BlockingProvisioner::new(vec![Ok(ready_inventory())]);
    provisioner.release(1);
    let runtime = runtime(Arc::clone(&provisioner), Arc::clone(&activity));
    runtime.start().unwrap();
    assert!(runtime.wait_for_idle(Duration::from_secs(3)));
    let _download = activity.try_acquire(DownloadOwner::Youtube).unwrap();

    assert_eq!(
        runtime.request_repair().unwrap_err(),
        ToolProvisioningErrorCode::DownloadInProgress
    );
    assert_eq!(runtime.status().state(), ToolProvisioningState::Ready);
    assert_eq!(provisioner.modes(), [ProvisioningMode::Startup]);
}

#[test]
fn failed_repair_returns_an_error_and_rechecks_the_previous_ready_set() {
    let provisioner = BlockingProvisioner::new(vec![
        Ok(ready_inventory()),
        Err(error(ManagedToolErrorCode::ToolProbeFailed)),
        Ok(ready_inventory()),
    ]);
    provisioner.release(3);
    let runtime = runtime(Arc::clone(&provisioner), activity());
    runtime.start().unwrap();
    assert!(runtime.wait_for_idle(Duration::from_secs(3)));

    assert_eq!(
        runtime.request_repair().unwrap_err(),
        ToolProvisioningErrorCode::ToolProbeFailed
    );
    assert_eq!(runtime.status().state(), ToolProvisioningState::Ready);
    assert_eq!(runtime.status().error_code(), None);
    assert_eq!(
        provisioner.modes(),
        [
            ProvisioningMode::Startup,
            ProvisioningMode::Repair,
            ProvisioningMode::Startup,
        ]
    );
}

#[test]
fn failed_repair_becomes_failed_when_the_previous_set_no_longer_matches() {
    let provisioner = BlockingProvisioner::new(vec![
        Ok(ready_inventory()),
        Err(error(ManagedToolErrorCode::ToolIntegrityFailed)),
        Err(error(ManagedToolErrorCode::ToolMissing)),
    ]);
    provisioner.release(3);
    let runtime = runtime(Arc::clone(&provisioner), activity());
    runtime.start().unwrap();
    assert!(runtime.wait_for_idle(Duration::from_secs(3)));

    assert_eq!(
        runtime.request_repair().unwrap_err(),
        ToolProvisioningErrorCode::PackageInvalid
    );
    assert_eq!(runtime.status().state(), ToolProvisioningState::Failed);
    assert_eq!(
        runtime.status().error_code(),
        Some(ToolProvisioningErrorCode::PackageInvalid)
    );
    assert!(!runtime.is_ready());
}

fn runtime(
    provisioner: Arc<BlockingProvisioner>,
    activity: Arc<DownloadActivityGate>,
) -> ToolProvisioningRuntime {
    ToolProvisioningRuntime::new(provisioner, activity)
}

fn activity() -> Arc<DownloadActivityGate> {
    Arc::new(DownloadActivityGate::new())
}

fn ready_inventory() -> ToolInventory {
    ToolInventory {
        target: BUILD_TARGET.to_owned(),
        tools: ToolName::required()
            .into_iter()
            .map(|name| ToolStatus {
                name,
                state: ToolState::Ready,
                version: Some("test-version".to_owned()),
            })
            .collect(),
    }
}

fn error(code: ManagedToolErrorCode) -> ManagedToolError {
    ManagedToolError::new(code, "private fixture detail")
}
