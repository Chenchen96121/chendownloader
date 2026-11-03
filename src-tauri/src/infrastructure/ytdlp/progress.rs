use std::collections::HashMap;

use crate::domain::download::{
    DownloadErrorCode, DownloadMessageKey, DownloadProgress, DownloadProgressMetrics, DownloadState,
};

const PREFIX: &[u8] = b"LMD_PROGRESS\t";
const MAX_LINE_BYTES: usize = 4096;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub fn parse_progress_line(line: &[u8]) -> Result<Option<DownloadProgress>, DownloadErrorCode> {
    if !line.starts_with(PREFIX) {
        return Ok(None);
    }
    if line.len() > MAX_LINE_BYTES {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let text =
        std::str::from_utf8(&line[PREFIX.len()..]).map_err(|_| DownloadErrorCode::OutputInvalid)?;
    let mut fields = HashMap::new();
    for field in text.split('\t') {
        let (key, value) = field
            .split_once('=')
            .ok_or(DownloadErrorCode::OutputInvalid)?;
        if !matches!(
            key,
            "job" | "downloaded" | "total" | "estimate" | "speed" | "eta" | "status"
        ) || fields.insert(key, value).is_some()
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
    }
    if fields.len() != 7 {
        return Err(DownloadErrorCode::OutputInvalid);
    }

    let job_id = required(&fields, "job")?;
    let downloaded_bytes = optional_u64(required(&fields, "downloaded")?, true)?;
    let exact_total = optional_u64(required(&fields, "total")?, false)?;
    let estimated_total = optional_u64(required(&fields, "estimate")?, false)?;
    let total_bytes = exact_total.or(estimated_total);
    let speed_bytes_per_second = optional_f64(required(&fields, "speed")?)?;
    let eta_seconds = optional_u64(required(&fields, "eta")?, true)?;
    if eta_seconds.is_some_and(|value| value > 31_536_000) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    let (state, message_key) = match required(&fields, "status")? {
        "downloading" => (DownloadState::Downloading, DownloadMessageKey::Downloading),
        "finished" => (DownloadState::Processing, DownloadMessageKey::Processing),
        _ => return Err(DownloadErrorCode::OutputInvalid),
    };
    let percent = downloaded_bytes
        .zip(total_bytes)
        .map(|(downloaded, total)| ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0));

    DownloadProgress::new(
        job_id,
        state,
        DownloadProgressMetrics {
            downloaded_bytes,
            total_bytes,
            percent,
            speed_bytes_per_second,
            eta_seconds,
        },
        message_key,
    )
    .map(Some)
    .map_err(|_| DownloadErrorCode::OutputInvalid)
}

fn required<'a>(fields: &'a HashMap<&str, &str>, key: &str) -> Result<&'a str, DownloadErrorCode> {
    fields
        .get(key)
        .copied()
        .filter(|value| !value.is_empty())
        .ok_or(DownloadErrorCode::OutputInvalid)
}

fn optional_u64(value: &str, allow_zero: bool) -> Result<Option<u64>, DownloadErrorCode> {
    if value == "NA" {
        return Ok(None);
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if parsed > MAX_SAFE_INTEGER || (!allow_zero && parsed == 0) {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(Some(parsed))
}

fn optional_f64(value: &str) -> Result<Option<f64>, DownloadErrorCode> {
    if value == "NA" {
        return Ok(None);
    }
    let parsed = value
        .parse::<f64>()
        .map_err(|_| DownloadErrorCode::OutputInvalid)?;
    if !parsed.is_finite() || parsed < 0.0 || parsed > MAX_SAFE_INTEGER as f64 {
        return Err(DownloadErrorCode::OutputInvalid);
    }
    Ok(Some(parsed))
}

pub struct ProgressParser {
    job_id: Option<String>,
    last_downloaded_bytes: Option<u64>,
    completed_bytes: u64,
    stream_finished: bool,
}

impl ProgressParser {
    pub fn begin_source(&mut self) -> Result<(), DownloadErrorCode> {
        if !self.stream_finished {
            self.completed_bytes = self
                .completed_bytes
                .checked_add(self.last_downloaded_bytes.unwrap_or(0))
                .filter(|n| *n <= MAX_SAFE_INTEGER)
                .ok_or(DownloadErrorCode::OutputInvalid)?;
        }
        self.stream_finished = false;
        self.last_downloaded_bytes = None;
        Ok(())
    }
    pub fn new() -> Self {
        Self {
            job_id: None,
            last_downloaded_bytes: None,
            completed_bytes: 0,
            stream_finished: false,
        }
    }

    pub fn parse(&mut self, line: &[u8]) -> Result<Option<DownloadProgress>, DownloadErrorCode> {
        let Some(mut progress) = parse_progress_line(line)? else {
            return Ok(None);
        };
        let stream_finished = progress.state == DownloadState::Processing;
        if self
            .job_id
            .as_deref()
            .is_some_and(|job_id| job_id != progress.job_id)
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }
        if self.stream_finished && !stream_finished {
            self.last_downloaded_bytes = None;
            self.stream_finished = false;
        }
        if self.stream_finished
            || matches!((self.last_downloaded_bytes, progress.downloaded_bytes), (Some(previous), Some(current)) if current < previous)
        {
            return Err(DownloadErrorCode::OutputInvalid);
        }

        self.job_id.get_or_insert_with(|| progress.job_id.clone());
        let raw_downloaded = progress.downloaded_bytes;
        let raw_total = progress.total_bytes;
        progress.downloaded_bytes = add_completed(self.completed_bytes, raw_downloaded)?;
        progress.total_bytes = add_completed(self.completed_bytes, raw_total)?;
        progress.percent =
            progress
                .downloaded_bytes
                .zip(progress.total_bytes)
                .map(|(downloaded, total)| {
                    ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
                });

        if raw_downloaded.is_some() {
            self.last_downloaded_bytes = raw_downloaded;
        }
        if stream_finished {
            let completed = raw_downloaded.or(self.last_downloaded_bytes).unwrap_or(0);
            self.completed_bytes = self
                .completed_bytes
                .checked_add(completed)
                .filter(|value| *value <= MAX_SAFE_INTEGER)
                .ok_or(DownloadErrorCode::OutputInvalid)?;
            self.stream_finished = true;
            progress.state = DownloadState::Downloading;
            progress.message_key = DownloadMessageKey::Downloading;
        }
        Ok(Some(progress))
    }
}

fn add_completed(completed: u64, current: Option<u64>) -> Result<Option<u64>, DownloadErrorCode> {
    current
        .map(|current| {
            completed
                .checked_add(current)
                .filter(|value| *value <= MAX_SAFE_INTEGER)
                .ok_or(DownloadErrorCode::OutputInvalid)
        })
        .transpose()
}

impl Default for ProgressParser {
    fn default() -> Self {
        Self::new()
    }
}
