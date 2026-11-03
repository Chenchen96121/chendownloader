#![allow(dead_code)]
use local_media_downloader_lib::{
    application::{
        bilibili_source::{
            BilibiliAnalysisScope, BilibiliAnalyzedSource, BilibiliAnalyzer, BilibiliPageEvidence,
            BilibiliPageLoader, BilibiliRequestContext,
        },
        network_profile::NetworkProfile,
        process::{
            CancellationToken, ProcessEventSink, ProcessExit, ProcessFailureKind, ProcessRequest,
            ProcessSupervisor,
        },
        tool_resolver::{ManagedToolResolver, ResolvedTool},
    },
    domain::{
        bilibili::BilibiliVideoRequest,
        managed_tools::{ManagedToolError, ToolName},
        media::AnalysisErrorCode,
    },
    infrastructure::{bilibili::page::parse_bilibili_page, ytdlp::bilibili::YtDlpBilibiliAnalyzer},
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
pub const URL: &str = "https://www.bilibili.com/video/BV1xx411c7mD?p=2";
pub const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jL1sAAAAASUVORK5CYII=";
pub fn metadata() -> Value {
    serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/bilibili/ordinary-metadata.json"
    ))
    .unwrap()
}
pub fn page() -> Value {
    let html = include_str!("../../../tests/fixtures/bilibili/ordinary-page.html");
    serde_json::from_str(
        html.split_once("window.__INITIAL_STATE__=")
            .unwrap()
            .1
            .split_once(";(function")
            .unwrap()
            .0,
    )
    .unwrap()
}
pub fn audio_probe() -> Value {
    json!({"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"90","size":"2160000"},"streams":[{"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2,"channel_layout":"stereo","bit_rate":"192000"}]})
}
pub fn video_probe() -> Value {
    json!({"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"90","size":"39375000"},"streams":[{"codec_type":"video","codec_name":"h264","profile":"High","width":1920,"height":1080,"avg_frame_rate":"30/1","pix_fmt":"yuv420p","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","bit_rate":"3500000"}]})
}
pub struct FixtureState {
    pub page: Mutex<Value>,
    pub metadata: Mutex<Value>,
    pub video: Mutex<Value>,
    pub audio: Mutex<Value>,
    pub metadata_calls: AtomicUsize,
    pub probe_calls: AtomicUsize,
    pub profiles: Mutex<Vec<Arc<NetworkProfile>>>,
    pub change_page_after_metadata: Mutex<Option<Value>>,
}
impl Default for FixtureState {
    fn default() -> Self {
        Self {
            page: Mutex::new(page()),
            metadata: Mutex::new(metadata()),
            video: Mutex::new(video_probe()),
            audio: Mutex::new(audio_probe()),
            metadata_calls: AtomicUsize::new(0),
            probe_calls: AtomicUsize::new(0),
            profiles: Mutex::new(Vec::new()),
            change_page_after_metadata: Mutex::new(None),
        }
    }
}
pub struct Resolver;
impl ManagedToolResolver for Resolver {
    fn resolve_verified(&self, name: ToolName) -> Result<ResolvedTool, ManagedToolError> {
        Ok(ResolvedTool {
            name,
            version: if name == ToolName::YtDlp {
                "2026.08.19"
            } else {
                "9.0.2"
            }
            .into(),
            path: PathBuf::from(r"C:\managed").join(format!("{}.exe", name.as_str())),
        })
    }
}
#[derive(Clone)]
struct Pages(Arc<FixtureState>);
impl BilibiliPageLoader for Pages {
    fn inspect(
        &self,
        request: &BilibiliVideoRequest,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliPageEvidence, AnalysisErrorCode> {
        self.0.profiles.lock().unwrap().push(context.network());
        let html = format!(
            "<script>window.__INITIAL_STATE__={};</script>",
            self.0.page.lock().unwrap()
        );
        parse_bilibili_page(request, html.as_bytes())
    }
}
struct Process(Arc<FixtureState>);
impl ProcessSupervisor for Process {
    fn run(
        &self,
        request: ProcessRequest,
        _: CancellationToken,
        _: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        let value = if request.program.path().file_name().unwrap() == "yt-dlp.exe" {
            self.0.metadata_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(page) = self.0.change_page_after_metadata.lock().unwrap().take() {
                *self.0.page.lock().unwrap() = page;
            }
            std::fs::write(
                request.working_dir.join("consumed-page.dump"),
                format!(
                    "<script>window.__INITIAL_STATE__={};</script>",
                    self.0.page.lock().unwrap()
                ),
            )
            .unwrap();
            self.0.metadata.lock().unwrap().clone()
        } else {
            self.0.probe_calls.fetch_add(1, Ordering::SeqCst);
            if request
                .args
                .last()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("audio.m4s")
            {
                self.0.audio.lock().unwrap().clone()
            } else {
                self.0.video.lock().unwrap().clone()
            }
        };
        Ok(ProcessExit {
            code: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: vec![],
        })
    }
}
#[derive(Clone)]
pub struct Analyzer(pub Arc<FixtureState>);
impl BilibiliAnalyzer for Analyzer {
    fn analyze(
        &self,
        request: &BilibiliVideoRequest,
        scope: BilibiliAnalysisScope,
        context: &BilibiliRequestContext,
    ) -> Result<BilibiliAnalyzedSource, AnalysisErrorCode> {
        YtDlpBilibiliAnalyzer::new(Resolver, Process(self.0.clone()), Pages(self.0.clone()))
            .analyze(request, scope, context)
    }
}
