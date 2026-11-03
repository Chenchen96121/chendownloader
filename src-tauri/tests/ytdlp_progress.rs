use local_media_downloader_lib::{
    domain::download::{DownloadErrorCode, DownloadMessageKey, DownloadState},
    infrastructure::ytdlp::progress::{parse_progress_line, ProgressParser},
};

fn line(fields: &[&str]) -> Vec<u8> {
    format!("LMD_PROGRESS\t{}", fields.join("\t")).into_bytes()
}

fn valid(
    downloaded: &str,
    total: &str,
    estimate: &str,
    speed: &str,
    eta: &str,
    status: &str,
) -> Vec<u8> {
    line(&[
        "job=42",
        &format!("downloaded={downloaded}"),
        &format!("total={total}"),
        &format!("estimate={estimate}"),
        &format!("speed={speed}"),
        &format!("eta={eta}"),
        &format!("status={status}"),
    ])
}

#[test]
fn ignores_unrelated_lines_and_parses_bounded_download_progress() {
    assert_eq!(
        parse_progress_line(b"[download] private diagnostic").unwrap(),
        None
    );
    let progress = parse_progress_line(&valid("250", "1000", "NA", "125.5", "6", "downloading"))
        .unwrap()
        .unwrap();
    assert_eq!(progress.job_id, "42");
    assert_eq!(progress.state, DownloadState::Downloading);
    assert_eq!(progress.downloaded_bytes, Some(250));
    assert_eq!(progress.total_bytes, Some(1000));
    assert_eq!(progress.percent, Some(25.0));
    assert_eq!(progress.speed_bytes_per_second, Some(125.5));
    assert_eq!(progress.eta_seconds, Some(6));
    assert_eq!(progress.message_key, DownloadMessageKey::Downloading);
}

#[test]
fn accepts_missing_placeholders_uses_estimated_total_and_clamps_percent() {
    let estimated = parse_progress_line(&valid("250", "NA", "1000", "NA", "NA", "downloading"))
        .unwrap()
        .unwrap();
    assert_eq!(estimated.total_bytes, Some(1000));
    assert_eq!(estimated.percent, Some(25.0));
    assert_eq!(estimated.speed_bytes_per_second, None);
    assert_eq!(estimated.eta_seconds, None);

    let clamped = parse_progress_line(&valid("1100", "1000", "2000", "0", "0", "downloading"))
        .unwrap()
        .unwrap();
    assert_eq!(clamped.total_bytes, Some(1000));
    assert_eq!(clamped.percent, Some(100.0));

    let processing = parse_progress_line(&valid("NA", "NA", "NA", "NA", "NA", "finished"))
        .unwrap()
        .unwrap();
    assert_eq!(processing.state, DownloadState::Processing);
    assert_eq!(processing.message_key, DownloadMessageKey::Processing);
}

#[test]
fn rejects_invalid_utf8_huge_duplicate_extra_and_unbounded_fields() {
    let mut invalid_utf8 = b"LMD_PROGRESS\tjob=42\tdownloaded=".to_vec();
    invalid_utf8.push(0xff);
    for candidate in [
        invalid_utf8,
        [b"LMD_PROGRESS\t".as_slice(), &vec![b'x'; 4090]].concat(),
        line(&[
            "job=42",
            "job=43",
            "downloaded=1",
            "total=2",
            "estimate=NA",
            "speed=1",
            "eta=1",
            "status=downloading",
        ]),
        line(&[
            "job=42",
            "downloaded=1",
            "total=2",
            "estimate=NA",
            "speed=1",
            "eta=1",
            "status=downloading",
            "extra=1",
        ]),
        valid("9007199254740992", "NA", "NA", "NA", "NA", "downloading"),
        valid("1", "0", "NA", "NA", "NA", "downloading"),
        valid("1", "2", "NA", "NaN", "1", "downloading"),
        valid("1", "2", "NA", "1", "31536001", "downloading"),
        valid("1", "2", "NA", "1", "1", "private"),
    ] {
        assert_eq!(
            parse_progress_line(&candidate),
            Err(DownloadErrorCode::OutputInvalid)
        );
    }
}

#[test]
fn stateful_parser_rejects_within_stream_regression_and_accumulates_the_next_stream() {
    let mut parser = ProgressParser::new();
    assert!(parser
        .parse(&valid("500", "1000", "NA", "10", "50", "downloading"))
        .unwrap()
        .is_some());
    assert_eq!(
        parser.parse(&valid("499", "1000", "NA", "10", "50", "downloading")),
        Err(DownloadErrorCode::OutputInvalid)
    );

    let mut parser = ProgressParser::new();
    let first_finished = parser
        .parse(&valid("1000", "1000", "NA", "NA", "NA", "finished"))
        .unwrap()
        .unwrap();
    assert_eq!(first_finished.state, DownloadState::Downloading);
    assert_eq!(first_finished.message_key, DownloadMessageKey::Downloading);
    let second_stream = parser
        .parse(&valid("100", "500", "NA", "20", "20", "downloading"))
        .unwrap()
        .unwrap();
    assert_eq!(second_stream.downloaded_bytes, Some(1100));
    assert_eq!(second_stream.total_bytes, Some(1500));
    let second_finished = parser
        .parse(&valid("500", "500", "NA", "NA", "NA", "finished"))
        .unwrap()
        .unwrap();
    assert_eq!(second_finished.downloaded_bytes, Some(1500));
    assert_eq!(second_finished.total_bytes, Some(1500));
}
