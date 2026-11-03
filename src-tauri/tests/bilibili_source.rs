use local_media_downloader_lib::{
    application::{
        bilibili_source::{BilibiliPageLoader, BilibiliRequestContext},
        network_profile::NetworkProfile,
        process::CancellationToken,
    },
    domain::{bilibili::validate_bilibili_url, media::AnalysisErrorCode},
    infrastructure::bilibili::{
        page::{
            parse_bilibili_page, BilibiliPageResponse, BilibiliPageTransport,
            HttpBilibiliPageLoader,
        },
        url_policy::{validate_bilibili_asset_url, BilibiliAssetKind},
    },
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use url::Url;

const PAGE: &[u8] = include_bytes!("../../tests/fixtures/bilibili/ordinary-page.html");
fn state() -> Value {
    let text = std::str::from_utf8(PAGE).unwrap();
    serde_json::from_str(
        text.split_once("window.__INITIAL_STATE__=")
            .unwrap()
            .1
            .split_once(";(function")
            .unwrap()
            .0,
    )
    .unwrap()
}
fn html(value: &Value) -> Vec<u8> {
    format!("<script>window.__INITIAL_STATE__={value};</script>").into_bytes()
}
fn context() -> BilibiliRequestContext {
    BilibiliRequestContext::new(
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    )
}

#[test]
fn bilibili_canonical_slash_redirect_is_followed_without_losing_selected_part() {
    struct SlashTransport(Arc<Mutex<Vec<String>>>);
    impl BilibiliPageTransport for SlashTransport {
        fn get(
            &self,
            url: &Url,
            _: &BilibiliRequestContext,
            _: Instant,
        ) -> Result<BilibiliPageResponse, AnalysisErrorCode> {
            self.0.lock().unwrap().push(url.as_str().into());
            Ok(if url.path().ends_with('/') {
                assert_eq!(url.query(), Some("p=2"));
                BilibiliPageResponse {
                    status: 200,
                    location: None,
                    body: PAGE.into(),
                }
            } else {
                BilibiliPageResponse {
                    status: 301,
                    location: Some("/video/BV1xx411c7mD/".into()),
                    body: vec![],
                }
            })
        }
    }
    let calls = Arc::new(Mutex::new(vec![]));
    let result = HttpBilibiliPageLoader::with_transport(SlashTransport(calls.clone())).inspect(
        &validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap(),
        &context(),
    );
    assert!(
        result.is_ok(),
        "validated same-site slash redirect must not be recanonicalized into a loop"
    );
    assert_eq!(result.unwrap().identity().cid, "279787");
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "https://www.bilibili.com/video/BV1xx411c7mD?p=2",
            "https://www.bilibili.com/video/BV1xx411c7mD/?p=2"
        ]
    );
}

#[test]
fn bilibili_part_identity_is_not_reinterpreted() {
    let default = validate_bilibili_url("https://bilibili.com/video/BV1xx411c7mD#share").unwrap();
    assert_eq!(default.part(), 1);
    assert_eq!(
        default.page_url(),
        "https://www.bilibili.com/video/BV1xx411c7mD?p=1"
    );
    for p in [
        "p=02",
        "%70=2",
        "p=%32",
        "P=2",
        "p=2&p=1",
        "p=2&%70=2",
        "p=0",
        "p=10001",
        "p=",
        "p=-1",
        "p=1.5",
        "p=18446744073709551616",
    ] {
        assert!(
            validate_bilibili_url(&format!("https://www.bilibili.com/video/av170001?{p}")).is_err(),
            "{p}"
        );
    }
    let request =
        validate_bilibili_url("https://www.bilibili.com/video/av170001?p=2&spm_id_from=share")
            .unwrap();
    let evidence = parse_bilibili_page(&request, PAGE).unwrap();
    assert_eq!(evidence.identity().bvid, "BV1xx411c7mD");
    assert_eq!(evidence.identity().aid, "170001");
    assert_eq!(evidence.identity().part, 2);
    assert_eq!(evidence.identity().cid, "279787");
    assert_eq!(evidence.identity().duration_seconds, 90.0);
    assert_eq!(
        evidence.source().url(),
        "https://www.bilibili.com/video/BV1xx411c7mD?p=2"
    );
    assert!(parse_bilibili_page(
        &validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=3").unwrap(),
        PAGE
    )
    .is_err());
    assert!(parse_bilibili_page(
        &validate_bilibili_url("https://www.bilibili.com/video/av170002?p=2").unwrap(),
        PAGE
    )
    .is_err());
}

#[test]
fn bilibili_rejects_disguised_and_out_of_scope_urls() {
    for url in [
        "http://www.bilibili.com/video/BV1xx411c7mD",
        "https://b23.tv/abc",
        "https://www.bilibili.com:443/video/BV1xx411c7mD",
        "https://u@www.bilibili.com/video/BV1xx411c7mD",
        "https://@www.bilibili.com/video/BV1xx411c7mD",
        "https://www.bilibili.com.evil.test/video/BV1xx411c7mD",
        "https://127.0.0.1/video/BV1xx411c7mD",
        "https://www.bilibili.com/foo/../video/BV1xx411c7mD",
        "https://www.bilibili.com/%76ideo/BV1xx411c7mD",
        "https://www.bilibili.com/video/%42V1xx411c7mD",
        "https://www.bilibili.com/video/bv1xx411c7mD",
        "https://www.bilibili.com/video/av0",
        "https://www.bilibili.com/video/av1.5",
        "https://www.bilibili.com/video/BV1xx411c7mD/x",
        "https://www.bilibili.com/bangumi/play/ep1",
        "https://www.bilibili.com\\video\\BV1xx411c7mD",
        "https://www.bilibili.com/video/BV1xx411c7mD\n",
        "https://www.bilibili.com/video/BV1xx411c7mD?x=\u{7f}",
    ] {
        assert!(validate_bilibili_url(url).is_err(), "{url:?}");
    }
    assert!(validate_bilibili_url(&format!(
        "https://www.bilibili.com/video/BV1xx411c7mD?x={}",
        "a".repeat(2048)
    ))
    .is_err());
    assert_eq!(
        validate_bilibili_url("https://www.bilibili.com/video/av9007199254740993?p=10000")
            .unwrap()
            .page_url(),
        "https://www.bilibili.com/video/av9007199254740993?p=10000"
    );
}

#[test]
fn bilibili_page_flags_override_popup_copy() {
    let request = validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap();
    assert!(parse_bilibili_page(&request, PAGE).is_ok());
    for (field, value) in [
        ("is_stein_gate", json!(1)),
        ("ugc_pay", json!(1)),
        ("ugc_pay_preview", json!(1)),
        ("pay", json!(1)),
        ("movie", json!(1)),
    ] {
        let mut page = state();
        page["videoData"]["rights"][field] = value;
        assert!(
            parse_bilibili_page(&request, &html(&page)).is_err(),
            "{field}"
        );
    }
    let mut page = state();
    page["videoData"]["is_upower_exclusive"] = true.into();
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let mut page = state();
    page["videoData"]["rights"]["ugc_pay"] = "unknown".into();
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let mut page = state();
    page["videoData"]["redirect_url"] = "https://www.bilibili.com/bangumi/play/ep1".into();
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    assert!(parse_bilibili_page(&request, b"<div>challenge: login</div>").is_err());
}

#[test]
fn bilibili_page_error_uses_pinned_true_code_without_requiring_a_success_flag() {
    let request = validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap();
    for error in [json!({}), json!({"trueCode":0})] {
        let mut page = state();
        page["error"] = error;
        assert!(parse_bilibili_page(&request, &html(&page)).is_ok());
        page["videoData"]["rights"]["pay"] = 1.into();
        assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    }
    for (code, expected) in [
        (-403, AnalysisErrorCode::AuthenticationRequired),
        (-404, AnalysisErrorCode::ContentUnavailable),
    ] {
        let mut page = state();
        page["error"] = json!({"trueCode":code});
        assert!(
            matches!(parse_bilibili_page(&request, &html(&page)), Err(found) if found==expected)
        );
    }
}

#[test]
fn bilibili_conflicting_restriction_flags_cannot_mask_each_other() {
    let request = validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap();
    let mut page = state();
    page["videoData"]["rights"]["is_upower_exclusive"] = false.into();
    page["videoData"]["is_upower_exclusive"] = true.into();
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let mut page = state();
    page["videoData"]["pay"] = 1.into();
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
}

#[test]
fn bilibili_page_accepts_exact_limits_without_parsing_other_part_media() {
    let request =
        validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=10000").unwrap();
    let mut page = state();
    page["videoData"]["pages"] = Value::Array(
        (1..=10000)
            .map(|n| json!({"page":n,"cid":n,"duration":90}))
            .collect(),
    );
    let mut body = html(&page);
    body.resize(4 * 1024 * 1024, b' ');
    let evidence = parse_bilibili_page(&request, &body).unwrap();
    assert_eq!(evidence.identity().part, 10000);
    assert_eq!(evidence.identity().cid, "10000");
    body.push(b' ');
    assert!(parse_bilibili_page(&request, &body).is_err());
}

#[test]
fn bilibili_only_actual_script_elements_can_supply_initial_state() {
    let request = validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap();
    let page = format!("<scripted>window.__INITIAL_STATE__={};</script>", state());
    assert!(parse_bilibili_page(&request, page.as_bytes()).is_err());
}

#[test]
fn bilibili_page_order_must_agree_with_the_pinned_extractor_cid_index() {
    let request = validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap();
    let mut page = state();
    page["videoData"]["pages"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let mut page = state();
    page["videoData"]["pages"].as_array_mut().unwrap().remove(0);
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
}

#[test]
fn bilibili_page_requires_unique_bounded_data_and_exact_numeric_ids() {
    let request = validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap();
    for value in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("0279787"),
        json!(null),
    ] {
        let mut page = state();
        page["videoData"]["pages"][1]["cid"] = value;
        assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    }
    let mut page = state();
    page["videoData"]["pages"][1]["cid"] = "9007199254740993".into();
    assert_eq!(
        parse_bilibili_page(&request, &html(&page))
            .unwrap()
            .identity()
            .cid,
        "9007199254740993"
    );
    let mut page = state();
    page["videoData"]["pages"][0]["page"] = 2.into();
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let mut page = state();
    page["videoData"]["pages"][1]["duration"] = 0.into();
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let mut page = state();
    page["videoData"]["pages"] = json!([]);
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let mut page = state();
    page["videoData"]["pages"] = Value::Array(
        (1..=10001)
            .map(|n| json!({"page":n,"cid":n,"duration":90}))
            .collect(),
    );
    assert!(parse_bilibili_page(&request, &html(&page)).is_err());
    let twice = [PAGE, PAGE].concat();
    assert!(parse_bilibili_page(&request, &twice).is_err());
    let duplicate = String::from_utf8(html(&state()))
        .unwrap()
        .replace("\"aid\":170001", "\"aid\":170002,\"aid\":170001");
    assert!(parse_bilibili_page(&request, duplicate.as_bytes()).is_err());
    let oversized = [PAGE, &vec![b' '; 4 * 1024 * 1024]].concat();
    assert!(parse_bilibili_page(&request, &oversized).is_err());
}

#[test]
fn bilibili_mcdn_https_8082_is_a_media_only_authority_exception() {
    let signed = "https://mirror.mcdn.bilivideo.cn:8082/video.m4s?x=%2f%2B&deadline=123&sign=a%3db";
    assert_eq!(
        validate_bilibili_asset_url(signed, BilibiliAssetKind::Media)
            .unwrap()
            .as_str(),
        signed
    );
    assert!(validate_bilibili_asset_url(signed, BilibiliAssetKind::Cover).is_err());
    for bad in [
        "http://mirror.mcdn.bilivideo.cn:8082/video.m4s",
        "https://mirror.mcdn.bilivideo.cn:8081/video.m4s",
        "https://mirror.mcdn.bilivideo.cn:08082/video.m4s",
        "https://mirror.mcdn.bilivideo.cn:443/video.m4s",
        "https://mcdn.bilivideo.cn:8082/video.m4s",
        "https://mirror.bilivideo.cn:8082/video.m4s",
        "https://mirror.mcdn.bilivideo.cn.evil.test:8082/video.m4s",
        "https://evil-mcdn.bilivideo.cn:8082/video.m4s",
        "https://user@mirror.mcdn.bilivideo.cn:8082/video.m4s",
        "https://mirror.mcdn.bilivideo.cn:8082/a/../video.m4s",
        "https://mirror.mcdn.bilivideo.cn:8082/%76ideo.m4s",
        "https://mirror.mcdn.bilivideo.cn:8082/video.m4s#fragment",
        "https://mirror.mcdn.bilivideo.cn:8082/video.m4s\n",
    ] {
        assert!(
            validate_bilibili_asset_url(bad, BilibiliAssetKind::Media).is_err(),
            "{bad:?}"
        );
    }
    assert!(validate_bilibili_url("https://www.bilibili.com:8082/video/BV13Bab6QEir/").is_err());
}

#[test]
fn bilibili_asset_policy_uses_label_boundaries_and_preserves_signature() {
    for kind in [BilibiliAssetKind::Cover, BilibiliAssetKind::Media] {
        for host in [
            "localhost",
            "127.0.0.1",
            "bilivideo.com.evil.test",
            "evilbilivideo.com",
            "hdslb.com.evil.test",
        ] {
            assert!(validate_bilibili_asset_url(&format!("https://{host}/asset"), kind).is_err());
        }
    }
    assert_eq!(
        validate_bilibili_asset_url(
            "http://i0.hdslb.com/image.jpg?x=1",
            BilibiliAssetKind::Cover
        )
        .unwrap()
        .as_str(),
        "https://i0.hdslb.com/image.jpg?x=1"
    );
    let signed = "https://upos-sz.bilivideo.com/video.m4s?x=%2f%2B&deadline=123&sign=a%3db";
    assert_eq!(
        validate_bilibili_asset_url(signed, BilibiliAssetKind::Media)
            .unwrap()
            .as_str(),
        signed
    );
    for bad in [
        "http://a.bilivideo.com/x",
        "https://a.bilivideo.com:443/x",
        "https://u@a.bilivideo.com/x",
        "https://a.bilivideo.com/x#fragment",
        "https://a.bilivideo.com\\x",
    ] {
        assert!(validate_bilibili_asset_url(bad, BilibiliAssetKind::Media).is_err());
    }
    assert!(
        validate_bilibili_asset_url("https://a.bilivideo.cn/x", BilibiliAssetKind::Media).is_ok()
    );
    assert!(
        validate_bilibili_asset_url("https://i0.hdslb.com/x", BilibiliAssetKind::Media).is_err()
    );
}

#[derive(Clone)]
struct Transport {
    responses: Arc<Mutex<VecDeque<BilibiliPageResponse>>>,
    requested: Arc<Mutex<Vec<String>>>,
}
impl BilibiliPageTransport for Transport {
    fn get(
        &self,
        url: &Url,
        context: &BilibiliRequestContext,
        deadline: Instant,
    ) -> Result<BilibiliPageResponse, AnalysisErrorCode> {
        assert!(deadline <= context.deadline());
        assert!(deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(15));
        self.requested.lock().unwrap().push(url.as_str().to_owned());
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request"))
    }
}
fn transport(responses: Vec<BilibiliPageResponse>) -> Transport {
    Transport {
        responses: Arc::new(Mutex::new(responses.into())),
        requested: Default::default(),
    }
}
fn ok() -> BilibiliPageResponse {
    BilibiliPageResponse {
        status: 200,
        location: None,
        body: PAGE.to_vec(),
    }
}
fn redirect(location: &str) -> BilibiliPageResponse {
    BilibiliPageResponse {
        status: 302,
        location: Some(location.into()),
        body: vec![],
    }
}

#[test]
fn bilibili_page_redirect_preserves_requested_part_and_checks_final_alias() {
    let t = transport(vec![
        redirect("https://www.bilibili.com/video/BV1xx411c7mD?p=1"),
        ok(),
    ]);
    let source = HttpBilibiliPageLoader::with_transport(t.clone())
        .inspect(
            &validate_bilibili_url("https://www.bilibili.com/video/av170001?p=2").unwrap(),
            &context(),
        )
        .unwrap();
    assert_eq!(source.identity().cid, "279787");
    assert_eq!(
        *t.requested.lock().unwrap(),
        vec![
            "https://www.bilibili.com/video/av170001?p=2",
            "https://www.bilibili.com/video/BV1xx411c7mD?p=2"
        ]
    );
    let t = transport(vec![redirect("/video/BV1xx411c7mD"), ok()]);
    assert!(HttpBilibiliPageLoader::with_transport(t)
        .inspect(
            &validate_bilibili_url("https://www.bilibili.com/video/av170002?p=2").unwrap(),
            &context()
        )
        .is_err());
}

#[test]
fn bilibili_page_stops_before_forbidden_redirect_target_or_fourth_hop() {
    for location in [
        "https://evil.test/video/BV1xx411c7mD",
        "https://passport.bilibili.com/login",
        "/bangumi/play/ep1",
        "/x/../video/BV1xx411c7mD",
        "https://www.bilibili.com:443/video/BV1xx411c7mD",
        "https://www.bilibili.com\\video\\BV1xx411c7mD",
    ] {
        let t = transport(vec![redirect(location)]);
        assert!(HttpBilibiliPageLoader::with_transport(t.clone())
            .inspect(
                &validate_bilibili_url("https://www.bilibili.com/video/av170001?p=2").unwrap(),
                &context()
            )
            .is_err());
        assert_eq!(t.requested.lock().unwrap().len(), 1, "{location}");
    }
    let t = transport(
        (0..4)
            .map(|_| redirect("/video/BV1xx411c7mD?p=2"))
            .collect(),
    );
    assert!(HttpBilibiliPageLoader::with_transport(t.clone())
        .inspect(
            &validate_bilibili_url("https://www.bilibili.com/video/av170001?p=2").unwrap(),
            &context()
        )
        .is_err());
    assert_eq!(t.requested.lock().unwrap().len(), 4); // initial request + 3 allowed hops
}

#[test]
fn bilibili_page_cancellation_and_deadline_prevent_any_request() {
    let t = transport(vec![]);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let c = BilibiliRequestContext::new(
        cancel,
        Instant::now() + Duration::from_secs(120),
        Arc::new(NetworkProfile::current()),
    );
    let r = validate_bilibili_url("https://www.bilibili.com/video/BV1xx411c7mD").unwrap();
    assert!(HttpBilibiliPageLoader::with_transport(t.clone())
        .inspect(&r, &c)
        .is_err());
    let c = BilibiliRequestContext::new(
        CancellationToken::new(),
        Instant::now() - Duration::from_secs(1),
        Arc::new(NetworkProfile::current()),
    );
    assert!(matches!(
        HttpBilibiliPageLoader::with_transport(t.clone()).inspect(&r, &c),
        Err(AnalysisErrorCode::AnalysisTimeout)
    ));
    assert!(t.requested.lock().unwrap().is_empty());
}
