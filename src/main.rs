#![windows_subsystem = "windows"]

mod client;
mod download;
mod error;
mod hardware;
mod panel;
mod process;
mod stats;
mod tray;
mod tunnel;
mod workspace;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{error, info, warn};

use crate::client::WorkerRole;
use crate::error::{WorkerError, Result};
use crate::hardware::HardwareInfo;
use crate::process::LlamaProcess;
use crate::stats::WorkerStats;

const RPC_PORT: u16 = 8090;
const LLAMA_PORT: u16 = 8081;

const ORACLE_HOST: &str = "159.54.175.236";
const ORACLE_PORT: u16 = 7000;

fn get_env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn get_env_or_u16(key: &str, default: u16) -> u16 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

const MASTER_HOST: &str = "159.54.175.236";
const MASTER_PORT: u16 = 7001;

/// Tiempo máximo para esperar a que el Maestro envíe INIT_MODEL
/// y el Worker descargue el modelo (2 horas para ~4.5 GB Qwen 7B).
const MODEL_TIMEOUT_SECS: u64 = 7200;

fn main() {
    if let Err(e) = run() {
        error!("Error fatal: {}", e);
        show_error_box(&format!("Worker Node — Error fatal\n\n{}", e));
    }
}

fn check_single_instance() -> Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::synchapi::CreateMutexW;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::shared::winerror::ERROR_ALREADY_EXISTS;

    let name: Vec<u16> = OsStr::new("Global\\EnjambreWorkerNode")
        .encode_wide()
        .chain(Some(0))
        .collect();

    unsafe {
        let mutex = CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr());
        if mutex.is_null() {
            return Err(WorkerError::Other("Error creando mutex de instancia única".into()));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Err(WorkerError::Other(
                "Ya hay una instancia de Worker Node ejecutándose.\n\n\
                 Revisa la bandeja del sistema (icono de Enjambre).\n\
                 Si no funciona, usa 'taskkill /F /IM worker-node.exe'.".into()
            ));
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    check_single_instance()?;

    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap())
        .join("WorkerNode")
        .join("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_appender = tracing_appender::rolling::daily(&log_dir, "worker-node");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("worker_node=info".parse().unwrap()),
        )
        .init();

    info!("=== Worker Node v0.5.1 (Super Node) iniciado ===");
    info!("Logs: {}", log_dir.display());

    let ws = workspace::Workspace::init()?;
    let hardware = hardware::detect();
    let stats = Arc::new(WorkerStats::new());

    let shutdown = Arc::new(AtomicBool::new(false));
    let state = Arc::new(AppState {
        rpc_process: tokio::sync::Mutex::new(None),
        server_process: tokio::sync::Mutex::new(None),
        hardware: hardware.clone(),
        shutdown: shutdown.clone(),
        stats: stats.clone(),
        session_start: std::time::Instant::now(),
    });

    ws.extract_llama()?;
    info!("Motor extraído en {}", ws.root.display());

    // ── Conectar al Maestro y obtener configuración ──────────────────
    let rt = tokio::runtime::Runtime::new()?;

    let (model_tx, model_rx) = tokio::sync::oneshot::channel::<client::ModelConfig>();
    let (slaves_tx, slaves_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    rt.spawn(client::start_master_connection(
        MASTER_HOST.to_string(),
        MASTER_PORT,
        hardware.clone(),
        RPC_PORT,
        model_tx,
        slaves_tx,
        ws.root.clone(),
    ));

    info!("Esperando configuración del Maestro {MASTER_HOST}:{MASTER_PORT}...");

    let model_config = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(MODEL_TIMEOUT_SECS), model_rx).await
        })
        .map_err(|_| WorkerError::Download(
            "Tiempo de espera agotado esperando modelo del Maestro (7200s)".to_string()
        ))?
        .map_err(|_| WorkerError::Download(
            "El Maestro no envió configuración de modelo".to_string()
        ))?;

    let role = model_config.role.clone();
    info!("Rol asignado por Maestro: {:?}", role);

    // ── Launch RPC server (SUPER_NODE y GPU_PROVIDER tienen GPU) ──
    if role == WorkerRole::SuperNode || role == WorkerRole::GpuProvider {
        let rpc_args = hardware::build_rpc_args(&hardware, RPC_PORT);
        info!("ggml-rpc-server args: {:?}", rpc_args);
        let rpc_proc = match LlamaProcess::spawn(&ws.rpc_bin, &rpc_args, "ggml-rpc-server", None) {
            Ok(proc) => {
                info!("ggml-rpc-server lanzado (PID: {})", proc.pid());
                proc
            }
            Err(e) => {
                error!("Error al lanzar ggml-rpc-server: {}", e);
                show_error_box(&format!("No se pudo iniciar ggml-rpc-server.\n\n{}", e));
                return Err(e.into());
            }
        };
        *state.rpc_process.blocking_lock() = Some(rpc_proc);

        std::thread::sleep(Duration::from_millis(500));
    } else {
        info!("CONSUMER: saltando ggml-rpc-server (sin GPU local)");
    }

    // ── Launch llama-server (solo SUPER_NODE) ───────────────────────
    if role == WorkerRole::SuperNode {
        let model_path = ws.root.join(format!("{}.gguf", model_config.name));
        let server_args = hardware::build_llama_server_args(
            &hardware,
            RPC_PORT,
            LLAMA_PORT,
            &model_path.to_string_lossy(),
        );
        info!("llama-server args: {:?}", server_args);
        let stderr_path = log_dir.join("llama-server-stderr.log");
        let server_proc = match LlamaProcess::spawn(&ws.server_bin, &server_args, "llama-server", Some(&stderr_path)) {
            Ok(proc) => {
                info!("llama-server lanzado (PID: {})", proc.pid());
                proc
            }
            Err(e) => {
                error!("Error al lanzar llama-server: {}", e);
                show_error_box(&format!("No se pudo iniciar llama-server.\n\n{}", e));
                return Err(e.into());
            }
        };
        *state.server_process.blocking_lock() = Some(server_proc);
    }

    let modo = match role {
        WorkerRole::SuperNode => "SUPER_NODE (GPU/CPU)",
        WorkerRole::GpuProvider => "GPU_PROVIDER (RPC slave)",
        WorkerRole::Consumer => "CONSUMER (API only)",
    };
    info!(
        "Worker Node listo — modo {}, modelo: {}, túnel a Oracle:{}",
        modo,
        if role == WorkerRole::SuperNode { &model_config.name } else { "N/A" },
        ORACLE_PORT,
    );

    // ── Spawn monitor + slave listener + tunnel + heartbeats ────────
    // (el heartbeat del Maestro corre dentro de client::start_master_connection)
    if role == WorkerRole::SuperNode || role == WorkerRole::GpuProvider {
        rt.spawn(monitor_children(state.clone()));
    }

    if role == WorkerRole::SuperNode {
        let model_path = ws.root.join(format!("{}.gguf", model_config.name));
        rt.spawn(handle_slave_updates(
            state.clone(),
            slaves_rx,
            ws.server_bin,
            hardware.clone(),
            model_path,
        ));
    }

    // Túnel de inferencia — SOLO para SUPER_NODE (el único que tiene llama-server)
    if role == WorkerRole::SuperNode {
        info!("SUPER_NODE abriendo túnel de inferencia a {}:{}", ORACLE_HOST, ORACLE_PORT);
        rt.spawn(tunnel::run_reverse_tunnel(
            format!("{}:{}", ORACLE_HOST, ORACLE_PORT),
            format!("127.0.0.1:{}", LLAMA_PORT),
            shutdown.clone(),
            stats.clone(),
        ));
    }

    // Túnel RPC — para GPU_PROVIDER (expone su rpc-server al enjambre)
    if role == WorkerRole::GpuProvider {
        let tunnel_port = model_config.tunnel_port;
        if tunnel_port > 0 {
            info!("GPU_PROVIDER: túnel RPC reverso a {}:{} → localhost:{}", MASTER_HOST, tunnel_port, RPC_PORT);
            rt.spawn(tunnel::run_reverse_tunnel(
                format!("{}:{}", MASTER_HOST, tunnel_port),
                format!("127.0.0.1:{}", RPC_PORT),
                shutdown.clone(),
                stats.clone(),
            ));
        } else {
            warn!("GPU_PROVIDER: tunnel_port=0, no se establece túnel RPC");
        }
    }

    // CONSUMER no abre túneles — usa la API HTTP directa al relay
    if role == WorkerRole::Consumer {
        info!("CONSUMER: conectado al relay para uso de API (sin túneles locales)");
    }

    // ── Tray (bloquea hasta que el usuario salga) ───────────────────
    tray::run(&state);

    info!("Cerrando Worker Node...");
    shutdown.store(true, Ordering::SeqCst);
    drop(rt);
    state.rpc_process.blocking_lock().take();
    state.server_process.blocking_lock().take();

    info!("Worker Node v0.5.1 finalizado.");
    Ok(())
}

async fn monitor_children(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));

    loop {
        interval.tick().await;
        if state.shutdown.load(Ordering::SeqCst) {
            break;
        }

        {
            let mut guard = state.rpc_process.lock().await;
            if let Some(proc) = guard.as_mut() {
                match proc.try_wait() {
                    Ok(Some(status)) => {
                        warn!("ggml-rpc-server terminó inesperadamente: {}", status);
                        guard.take();
                        state.shutdown.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(None) => {}
                    Err(e) => error!("Error monitoreando ggml-rpc-server: {}", e),
                }
            }
        }

        {
            let mut guard = state.server_process.lock().await;
            if let Some(proc) = guard.as_mut() {
                match proc.try_wait() {
                    Ok(Some(status)) => {
                        warn!("llama-server terminó inesperadamente: {}", status);
                        guard.take();
                        state.shutdown.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(None) => {}
                    Err(e) => error!("Error monitoreando llama-server: {}", e),
                }
            }
        }
    }
}

/// Escucha cambios en la lista de esclavos RPC (desde el Maestro vía
/// UPDATE_SLAVES) y relanza llama-server con la nueva cadena --rpc.
async fn handle_slave_updates(
    state: Arc<AppState>,
    mut slaves_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    server_bin: std::path::PathBuf,
    hardware: HardwareInfo,
    model_path: std::path::PathBuf,
) {
    while let Some(slaves) = slaves_rx.recv().await {
        info!("Recibida actualización de esclavos RPC: {slaves}");

        let local_rpc = format!("127.0.0.1:{RPC_PORT}");
        let rpc_chain = if slaves.is_empty() {
            local_rpc
        } else {
            format!("{local_rpc},{slaves}")
        };

        let mut guard = state.server_process.lock().await;
        // Dropear el proceso anterior lo mata vía Job Object (KILL_ON_JOB_CLOSE)
        drop(guard.take());

        let args = vec![
            "--rpc".to_string(),
            rpc_chain.clone(),
            "-m".to_string(),
            model_path.to_string_lossy().to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            LLAMA_PORT.to_string(),
            "--threads".to_string(),
            hardware.used_threads.to_string(),
        ];

        info!("Relanzando llama-server con --rpc {rpc_chain}");
        let stderr_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap())
            .join("WorkerNode")
            .join("logs")
            .join("llama-server-stderr.log");
        match LlamaProcess::spawn(&server_bin, &args, "llama-server", Some(&stderr_path)) {
            Ok(proc) => {
                info!("llama-server relanzado (PID {})", proc.pid());
                *guard = Some(proc);
            }
            Err(e) => {
                warn!("Error relanzando llama-server: {e}");
            }
        }
        drop(guard);
    }
}

fn show_error_box(msg: &str) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(msg)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = std::ffi::OsStr::new("Worker Node — Error")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        winapi::um::winuser::MessageBoxW(
            std::ptr::null_mut(),
            wide.as_ptr(),
            title.as_ptr(),
            winapi::um::winuser::MB_OK | winapi::um::winuser::MB_ICONERROR,
        );
    }
}

pub(crate) struct AppState {
    pub rpc_process: tokio::sync::Mutex<Option<LlamaProcess>>,
    pub server_process: tokio::sync::Mutex<Option<LlamaProcess>>,
    pub hardware: HardwareInfo,
    pub shutdown: Arc<AtomicBool>,
    pub stats: Arc<WorkerStats>,
    pub session_start: std::time::Instant,
}
