use local_media_downloader_lib::{
    application::{
        download_activity::{DownloadActivityGate, DownloadOwner, TaskPhase},
        process::CancellationToken,
    },
    domain::download::DownloadErrorCode,
};
use std::{
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

#[test]
fn x_uses_shared_fifo_and_never_fabricates_a_whole_post_percent() {
    use local_media_downloader_lib::domain::{
        media::Platform,
        x::{XDownloadProgress, XJobState},
    };
    let gate = DownloadActivityGate::new();
    let yt = gate.reserve(DownloadOwner::Youtube, "yt").unwrap();
    let ins = gate.reserve(DownloadOwner::Instagram, "ins").unwrap();
    let x = gate.reserve(DownloadOwner::X, "x").unwrap();
    yt.wait_turn(&CancellationToken::new()).unwrap();
    ins.wait_turn(&CancellationToken::new()).unwrap();
    let id = x.job_id();
    assert_eq!(gate.snapshot().unwrap().tasks[2].platform, Platform::X);
    assert_eq!(gate.snapshot().unwrap().tasks[2].state, TaskPhase::Queued);
    drop(yt);
    x.wait_turn(&CancellationToken::new()).unwrap();
    drop(ins);
    assert_eq!(gate.active_owner(), Some(DownloadOwner::X));
    gate.record_x(&XDownloadProgress {
        job_id: id.clone(),
        state: XJobState::Downloading,
        item_index: 2,
        item_count: 4,
        downloaded_bytes: Some(50),
        total_bytes: Some(100),
        percent: Some(50.0),
    });
    let task = gate.snapshot().unwrap().tasks.pop().unwrap();
    assert_eq!(task.state, TaskPhase::Downloading);
    assert_eq!(task.percent, None);
    gate.record_x(&XDownloadProgress {
        job_id: id,
        state: XJobState::Completed,
        item_index: 4,
        item_count: 4,
        downloaded_bytes: None,
        total_bytes: None,
        percent: None,
    });
    assert_eq!(
        gate.snapshot().unwrap().tasks[2].state,
        TaskPhase::Completed
    );
    drop(x);
    assert!(gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
}

#[test]
fn shared_fifo_slots_bound_cross_platform_execution_and_release_independently() {
    let gate = Arc::new(DownloadActivityGate::new());
    let first = gate.reserve(DownloadOwner::Youtube, "YouTube A").unwrap();
    let second = gate
        .reserve(DownloadOwner::Instagram, "Instagram B")
        .unwrap();
    let third = gate.reserve(DownloadOwner::Youtube, "YouTube C").unwrap();
    let fourth = gate
        .reserve(DownloadOwner::Instagram, "Instagram D")
        .unwrap();
    assert_ne!(first.job_id(), second.job_id());
    first.wait_turn(&CancellationToken::new()).unwrap();
    second.wait_turn(&CancellationToken::new()).unwrap();
    let (tx, rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let third_worker = thread::spawn(move || {
        third.wait_turn(&CancellationToken::new()).unwrap();
        tx.send("third").unwrap();
        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    });
    let snapshot = gate.snapshot().unwrap();
    assert_eq!(snapshot.limit, 2);
    assert_eq!(
        snapshot
            .tasks
            .iter()
            .filter(|task| task.state == TaskPhase::Preparing)
            .count(),
        2
    );
    assert_eq!(
        snapshot
            .tasks
            .iter()
            .filter(|task| task.state == TaskPhase::Queued)
            .count(),
        2
    );
    assert_eq!(
        gate.try_acquire(DownloadOwner::ToolRepair).err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
    assert!(rx.recv_timeout(Duration::from_millis(80)).is_err());
    gate.set_limit(1).unwrap(); // Does not terminate either already-running task.
    drop(first);
    assert!(rx.recv_timeout(Duration::from_millis(80)).is_err());
    drop(second);
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "third");
    let (tx, rx) = mpsc::channel();
    let last_worker = thread::spawn(move || {
        fourth.wait_turn(&CancellationToken::new()).unwrap();
        tx.send(()).unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(80)).is_err());
    release_tx.send(()).unwrap();
    third_worker.join().unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    last_worker.join().unwrap();
    assert!(gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
}

#[test]
fn queue_cancel_close_and_capacity_are_bounded_without_acquiring_a_slot() {
    let gate = DownloadActivityGate::new();
    gate.set_limit(1).unwrap();
    let active = gate.reserve(DownloadOwner::Youtube, "active").unwrap();
    active.wait_turn(&CancellationToken::new()).unwrap();
    let queued = gate.reserve(DownloadOwner::Instagram, "queued").unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        queued.wait_turn(&cancel),
        Err(DownloadErrorCode::ProcessFailed)
    );
    assert_eq!(gate.snapshot().unwrap().tasks[1].state, TaskPhase::Queued);
    drop(queued);
    let mut waiting = vec![];
    for _ in 0..31 {
        waiting.push(gate.reserve(DownloadOwner::Instagram, "next").unwrap());
    }
    assert_eq!(
        gate.reserve(DownloadOwner::Youtube, "overflow").err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
    for value in [0, 5, u8::MAX] {
        assert_eq!(
            gate.set_limit(value),
            Err(DownloadErrorCode::InvalidRequest)
        );
    }
    assert_eq!(gate.snapshot().unwrap().limit, 1);
    gate.close();
    let cancellation = CancellationToken::new();
    assert_eq!(
        waiting[0].wait_turn(&cancellation),
        Err(DownloadErrorCode::ProcessFailed)
    );
    assert!(cancellation.is_canceled());
    assert_eq!(
        gate.reserve(DownloadOwner::Youtube, "after close").err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
    drop(waiting);
    drop(active);
}

#[test]
fn increased_limit_wakes_waiters_and_panics_release_the_exact_lease() {
    let gate = Arc::new(DownloadActivityGate::new());
    gate.set_limit(1).unwrap();
    let first = gate.reserve(DownloadOwner::Youtube, "one").unwrap();
    first.wait_turn(&CancellationToken::new()).unwrap();
    let second = gate.reserve(DownloadOwner::Instagram, "two").unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        second.wait_turn(&CancellationToken::new()).unwrap();
        tx.send(()).unwrap();
        panic!("fixture worker failure");
    });
    assert!(rx.recv_timeout(Duration::from_millis(80)).is_err());
    gate.set_limit(2).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(worker.join().is_err());
    assert_eq!(gate.snapshot().unwrap().tasks[1].state, TaskPhase::Failed);
    let third = gate.reserve(DownloadOwner::Instagram, "three").unwrap();
    third.wait_turn(&CancellationToken::new()).unwrap();
    drop(third);
    drop(first);
    assert!(gate.try_acquire(DownloadOwner::ToolRepair).is_ok());
}

#[test]
fn task_history_is_bounded_and_private_urls_are_not_part_of_the_snapshot() {
    let gate = DownloadActivityGate::new();
    for _ in 0..110 {
        let job = gate
            .reserve(DownloadOwner::Youtube, "display title")
            .unwrap();
        drop(job);
    }
    let snapshot = gate.snapshot().unwrap();
    assert_eq!(snapshot.tasks.len(), 100);
    assert_eq!(snapshot.tasks.first().unwrap().job_id, "11");
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("url"));
    assert!(!json.contains("path"));
    let repair = gate.try_acquire(DownloadOwner::ToolRepair).unwrap();
    assert_eq!(
        gate.reserve(DownloadOwner::Instagram, "blocked").err(),
        Some(DownloadErrorCode::DownloadInProgress)
    );
    drop(repair);
}
