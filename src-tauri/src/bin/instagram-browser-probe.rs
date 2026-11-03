use local_media_downloader_lib::{
    application::media_url::validate_media_url,
    domain::media::AnalysisErrorCode,
    infrastructure::instagram_browser::{
        parse_browser_capture, BrowserCaptureReport, MAX_CAPTURE_BYTES,
    },
};
use std::{env, fs::File, io::Read};

fn run() -> Result<BrowserCaptureReport, AnalysisErrorCode> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 4 || args[0] != "--url" || args[2] != "--input" {
        eprintln!("Usage: instagram-browser-probe --url https://www.instagram.com/p/SHORTCODE/ --input capture.json");
        return Err(AnalysisErrorCode::InvalidUrl);
    }
    let url = args[1].to_str().ok_or(AnalysisErrorCode::InvalidUrl)?;
    let source = validate_media_url(url)?;
    let file = File::open(&args[3]).map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CAPTURE_BYTES as u64 {
        return Err(AnalysisErrorCode::InvalidMetadata);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CAPTURE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AnalysisErrorCode::AnalysisUnavailable)?;
    parse_browser_capture(&source, &bytes)
}

fn main() {
    match run() {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("closed report serialization")
            );
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({ "captureOnly": true, "downloadEnabled": false, "error": error })
            );
            std::process::exit(2);
        }
    }
}
