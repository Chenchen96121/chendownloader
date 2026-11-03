use super::{x_video_http_client, XHttpPolicy};
use crate::{
    application::{process::CancellationToken, x_source::XRequestContext},
    infrastructure::system_proxy::SystemProxies,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn http_session_is_lazy_until_a_role_is_requested() {
    let network = SystemProxies::default().snapshot();
    assert!(network.x_video_http.manifest.get().is_none());
    assert!(network.x_video_http.segment.get().is_none());
    let _client = x_video_http_client(&network, XHttpPolicy::Manifest).unwrap();
    assert!(network.x_video_http.manifest.get().is_some());
    assert!(network.x_video_http.segment.get().is_none());
}

#[test]
fn http_session_reuses_role_across_clones_and_child_deadlines() {
    let network = Arc::new(SystemProxies::default().snapshot());
    let cloned = network.as_ref().clone();
    let parent = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        network.clone(),
    );
    let child = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(15),
        network.clone(),
    );
    for policy in [XHttpPolicy::Manifest, XHttpPolicy::Segment] {
        let first = x_video_http_client(&parent.network, policy).unwrap();
        let again = x_video_http_client(&child.network, policy).unwrap();
        let clone_client = x_video_http_client(&cloned, policy).unwrap();
        assert!(
            Arc::ptr_eq(&first, &again),
            "child deadline must not create a new pool"
        );
        assert!(
            Arc::ptr_eq(&first, &clone_client),
            "profile clones must keep their pool"
        );
    }
}

#[test]
fn http_session_never_shares_across_roles_or_fresh_snapshots() {
    let settings = SystemProxies::default();
    let network = settings.snapshot();
    let other = settings.snapshot();
    let manifest = x_video_http_client(&network, XHttpPolicy::Manifest).unwrap();
    let segment = x_video_http_client(&network, XHttpPolicy::Segment).unwrap();
    assert!(
        !Arc::ptr_eq(&manifest, &segment),
        "different transport policies must remain isolated"
    );
    assert!(!Arc::ptr_eq(
        &manifest,
        &x_video_http_client(&other, XHttpPolicy::Manifest).unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &segment,
        &x_video_http_client(&other, XHttpPolicy::Segment).unwrap()
    ));
}

#[test]
fn http_session_concurrent_initialization_produces_one_role_client() {
    let network = Arc::new(SystemProxies::default().snapshot());
    let clients = std::thread::scope(|scope| {
        let handles = (0..3)
            .map(|_| scope.spawn(|| x_video_http_client(&network, XHttpPolicy::Segment).unwrap()))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(clients[1..]
        .iter()
        .all(|client| Arc::ptr_eq(&clients[0], client)));
}

#[test]
fn http_session_canceling_one_context_does_not_poison_another() {
    let network = Arc::new(SystemProxies::default().snapshot());
    let first = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(15),
        network.clone(),
    );
    let sibling = XRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        network,
    );
    let client = x_video_http_client(&first.network, XHttpPolicy::Segment).unwrap();
    first.cancellation.cancel();
    assert!(!sibling.cancellation.is_canceled());
    assert!(Arc::ptr_eq(
        &client,
        &x_video_http_client(&sibling.network, XHttpPolicy::Segment).unwrap()
    ));
}

#[test]
fn http_session_cache_drops_with_its_last_profile_owner_without_a_cycle() {
    let network = SystemProxies::default().snapshot();
    let cache = Arc::downgrade(&network.x_video_http);
    let clone = network.clone();
    let client = x_video_http_client(&network, XHttpPolicy::Manifest).unwrap();
    drop(network);
    assert!(cache.upgrade().is_some());
    drop(clone);
    assert!(
        cache.upgrade().is_none(),
        "cached client must not retain the profile or pool"
    );
    drop(client);
}
