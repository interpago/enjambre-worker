use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

use crate::hardware::HardwareInfo;
use crate::process::LlamaProcess;
use crate::stats::WorkerStats;

pub struct AppState {
    pub rpc_process: tokio::sync::Mutex<Option<LlamaProcess>>,
    pub server_process: tokio::sync::Mutex<Option<LlamaProcess>>,
    pub hardware: HardwareInfo,
    pub shutdown: Arc<AtomicBool>,
    pub stats: Arc<WorkerStats>,
    pub session_start: std::time::Instant,
    pub master_credits: Arc<AtomicU64>,
}
