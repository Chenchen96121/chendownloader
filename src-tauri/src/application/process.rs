use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use super::tool_resolver::ResolvedTool;

pub struct ProcessProgram {
    path: PathBuf,
    system_proxy_allowed: bool,
    network_profile: Option<Arc<super::network_profile::NetworkProfile>>,
    explicit_https_proxy_host: Option<&'static str>,
}

impl ProcessProgram {
    pub(crate) fn with_explicit_https_proxy_for_host(mut self, host: &'static str) -> Self {
        self.explicit_https_proxy_host = Some(host);
        self
    }
    pub(crate) fn explicit_https_proxy_host(&self) -> Option<&'static str> {
        self.explicit_https_proxy_host
    }
    pub(crate) fn with_network_profile(
        mut self,
        profile: Arc<super::network_profile::NetworkProfile>,
    ) -> Self {
        self.network_profile = Some(profile);
        self
    }
    pub(crate) fn network_profile(&self) -> Option<&super::network_profile::NetworkProfile> {
        self.network_profile.as_deref()
    }
    // Keeping this crate-only prevents arbitrary frontend paths.
    pub(crate) fn from_resolved_tool(tool: ResolvedTool) -> Self {
        Self {
            path: tool.path,
            network_profile: None,
            explicit_https_proxy_host: None,
            system_proxy_allowed: matches!(
                tool.name,
                crate::domain::managed_tools::ToolName::YtDlp
                    | crate::domain::managed_tools::ToolName::GalleryDl
            ),
        }
    }

    pub(crate) fn from_verified_staged_candidate(path: PathBuf) -> Self {
        Self {
            path,
            network_profile: None,
            explicit_https_proxy_host: None,
            system_proxy_allowed: false,
        }
    }

    pub(crate) const fn system_proxy_allowed(&self) -> bool {
        self.system_proxy_allowed
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[doc(hidden)]
    #[cfg(feature = "process-fixture")]
    pub fn for_test_fixture(path: PathBuf) -> Self {
        Self::from_verified_staged_candidate(path)
    }
}

#[derive(Clone, Default)]
pub struct CancellationToken {
    canceled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

pub enum OutputMode {
    Capture {
        stdout_limit: usize,
        stderr_limit: usize,
    },
    Lines {
        line_limit: usize,
        retained_stderr_limit: usize,
    },
}

pub struct ProcessRequest {
    pub program: ProcessProgram,
    pub args: Vec<OsString>,
    pub working_dir: PathBuf,
    pub timeout: Option<Duration>,
    pub inactivity_timeout: Option<Duration>,
    pub output_mode: OutputMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessStream {
    Stdout,
    Stderr,
}

pub trait ProcessEventSink: Send + Sync {
    fn on_line(&self, stream: ProcessStream, line: &[u8]);
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProcessExit {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessFailureKind {
    LaunchFailed,
    JobControlFailed,
    IoFailed,
    StdoutLimitExceeded,
    StderrLimitExceeded,
    LineLimitExceeded,
    TimedOut,
    InactivityTimedOut,
    Canceled,
}

pub trait ProcessSupervisor: Send + Sync {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind>;
}

impl<T: ProcessSupervisor + ?Sized> ProcessSupervisor for &T {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        (**self).run(request, cancellation, sink)
    }
}

impl<T: ProcessSupervisor + ?Sized> ProcessSupervisor for Arc<T> {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: CancellationToken,
        sink: &dyn ProcessEventSink,
    ) -> Result<ProcessExit, ProcessFailureKind> {
        (**self).run(request, cancellation, sink)
    }
}
