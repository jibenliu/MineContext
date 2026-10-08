//! 采集控制面：把「采集源 + blob 存储 + 录制开关」聚合起来给 HTTP 层用。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mc_capture::source::CaptureSource;
use mc_storage::blob::FileSystemBlobStore;

pub struct CaptureControls {
    pub source: Arc<dyn CaptureSource>,
    pub blobs: Arc<FileSystemBlobStore>,
    running: AtomicBool,
}

impl CaptureControls {
    pub fn new(source: Arc<dyn CaptureSource>, blobs: Arc<FileSystemBlobStore>) -> Self {
        Self {
            source,
            blobs,
            running: AtomicBool::new(false),
        }
    }

    /// 当前挂载的采集源 id（诊断与启动日志用）。
    pub fn source_id(&self) -> String {
        self.source.id().to_string()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
