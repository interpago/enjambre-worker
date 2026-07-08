#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tracing::{error, info, warn};

use worker_node_core::client::{ModelConfig, WorkerRole};
use worker_node_core::error::{Result, WorkerError};
use worker_node_core::hardware::HardwareInfo;
use worker_node_core::process::LlamaProcess;
use worker_node_core::state::AppState;
use worker_node_core::stats::WorkerStats;
use worker_node_core::{hardware, tunnel, workspace};

// ─── Constants ─────────────────────────────────────────────────────────────────

const RPC_PORT: u16 = 8090;
const LLAMA_PORT: u16 = 8081;

const ORACLE_HOST: &str = "159.54.175.236";
const ORACLE_PORT: u16 = 7000;

const MASTER_HOST: &str = "159.54.175.236";
const MASTER_PORT: u16 = 7001;

const MODEL_TIMEOUT_SECS: u64 = 7200;

// ─── Global State ──────────────────────────────────────────────────────────────

static APP_STATE: std::sync::OnceLock<Arc<AppState>> = std::sync::OnceLock::new();

#[derive(serde::Serialize)]
pub struct PanelStatus {
    pub mode: String,
    pub credits: u64,
    pub total_bytes: u64,
    pub estimated_tokens: u64,
    pub session_secs: u64,
    pub hardware: String,
    pub vram_mib: String,
    pub ready: bool,
}

// ─── Tauri Commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn get_panel_status() -> PanelStatus {
    if let Some(state) = APP_STATE.get() {
        let stats = &state.stats;
        let bytes = stats.total_bytes();
        let tokens = stats.estimated_tokens();
        let credits = state.master_credits.load(Ordering::Relaxed);
        let elapsed = state.session_start.elapsed().as_secs();
        let mode = if state.hardware.has_nvidia_gpu { "GPU" } else { "CPU" };
        let hw = state
            .hardware
            .nvidia_gpu_name
            .clone()
            .unwrap_or_else(|| "CPU-only".to_string());
        let vram = state
            .hardware
            .vram_mib
            .map(|m| format!("{} MiB", m))
            .unwrap_or_else(|| "N/A".to_string());

        PanelStatus {
            mode: mode.to_string(),
            credits,
            total_bytes: bytes,
            estimated_tokens: tokens,
            session_secs: elapsed,
            hardware: hw,
            vram_mib: vram,
            ready: true,
        }
    } else {
        PanelStatus {
            mode: "Conectando...".to_string(),
            credits: 0,
            total_bytes: 0,
            estimated_tokens: 0,
            session_secs: 0,
            hardware: String::new(),
            vram_mib: String::new(),
            ready: false,
        }
    }
}

#[tauri::command]
fn toggle_panel(window: tauri::WebviewWindow) -> std::result::Result<(), String> {
    if window.is_visible().map_err(|e| e.to_string())? {
        window.hide().map_err(|e| e.to_string())?;
    } else {
        center_bottom_right(&window);
        window.show().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn center_bottom_right(window: &tauri::WebviewWindow) {
    if let Ok(Some(monitor)) = window.current_monitor() {
        let size = monitor.size();
        let w = 340;
        let h = 260;
        let x = (size.width as i32 - w).max(0);
        let y = (size.height as i32 - h - 50).max(0);
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

// ─── Tray Menu ─────────────────────────────────────────────────────────────────

fn build_tray_menu(app: &tauri::AppHandle) -> std::result::Result<Menu<tauri::Wry>, tauri::Error> {
    let show = MenuItem::with_id(app, "show", "Mostrar Panel", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Salir", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    Ok(menu)
}

// ─── Single Instance ───────────────────────────────────────────────────────────

fn check_single_instance() -> std::result::Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use winapi::shared::winerror::ERROR_ALREADY_EXISTS;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::synchapi::CreateMutexW;

    let name: Vec<u16> = OsStr::new("Global\\EnjambreWorkerNode")
        .encode_wide()
        .chain(Some(0))
        .collect();

    unsafe {
        let mutex = CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr());
        if mutex.is_null() {
            return Err("Error creando mutex de instancia única".into());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Err(
                "Ya hay una instancia de Worker Node ejecutándose.\n\n\
                 Revisa la bandeja del sistema (icono de Enjambre).\n\
                 Si no funciona, usa 'taskkill /F /IM worker-node.exe'."
                    .into(),
            );
        }
    }
    Ok(())
}

// ─── Worker Initialization ─────────────────────────────────────────────────────

fn start_worker_background(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        if let Err(e) = run_worker(&app_handle) {
            error!("Error fatal en worker: {}", e);
            let _ = app_handle.exit(1);
        }
    });
}

fn run_worker(_app_handle: &tauri::AppHandle) -> Result<()> {
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
                .add_directive("worker_node_core=info".parse().unwrap()),
        )
        .init();

    info!("=== Enjambre Worker Node v0.5.1 (Tauri) iniciado ===");
    info!("Logs: {}", log_dir.display());

    let ws = workspace::Workspace::init()?;
    let hw = hardware::detect();
    let stats = Arc::new(WorkerStats::new());

    let shutdown = Arc::new(AtomicBool::new(false));
    let master_credits = Arc::new(AtomicU64::new(0));
    let state = Arc::new(AppState {
        rpc_process: tokio::sync::Mutex::new(None),
        server_process: tokio::sync::Mutex::new(None),
        hardware: hw.clone(),
        shutdown: shutdown.clone(),
        stats: stats.clone(),
        session_start: std::time::Instant::now(),
        master_credits: master_credits.clone(),
    });
    let _ = APP_STATE.set(state.clone());

    ws.extract_llama()?;
    info!("Motor extraído en {}", ws.root.display());

    let rt = tokio::runtime::Runtime::new()?;

    let (model_tx, model_rx) = tokio::sync::oneshot::channel::<ModelConfig>();
    let (slaves_tx, slaves_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    rt.spawn(worker_node_core::client::start_master_connection(
        MASTER_HOST.to_string(),
        MASTER_PORT,
        hw.clone(),
        RPC_PORT,
        model_tx,
        slaves_tx,
        ws.root.clone(),
        master_credits,
    ));

    info!("Esperando configuración del Maestro {MASTER_HOST}:{MASTER_PORT}...");

    let model_config = rt
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(MODEL_TIMEOUT_SECS), model_rx).await
        })
        .map_err(|_| {
            WorkerError::Download(
                "Tiempo de espera agotado esperando modelo del Maestro (7200s)".to_string(),
            )
        })?
        .map_err(|_| {
            WorkerError::Download("El Maestro no envió configuración de modelo".to_string())
        })?;

    let role = model_config.role.clone();
    info!("Rol asignado por Maestro: {:?}", role);

    if role == WorkerRole::SuperNode || role == WorkerRole::GpuProvider {
        let rpc_args = hardware::build_rpc_args(&hw, RPC_PORT);
        info!("ggml-rpc-server args: {:?}", rpc_args);
        let rpc_proc =
            LlamaProcess::spawn(&ws.rpc_bin, &rpc_args, "ggml-rpc-server", None).map_err(
                |e| {
                    error!("Error al lanzar ggml-rpc-server: {}", e);
                    WorkerError::Other(format!("No se pudo iniciar ggml-rpc-server: {}", e))
                },
            )?;
        *state.rpc_process.blocking_lock() = Some(rpc_proc);
        std::thread::sleep(Duration::from_millis(500));
    } else {
        info!("CONSUMER: saltando ggml-rpc-server (sin GPU local)");
    }

    if role == WorkerRole::SuperNode {
        let model_path = ws.root.join(format!("{}.gguf", model_config.name));
        let server_args =
            hardware::build_llama_server_args(&hw, RPC_PORT, LLAMA_PORT, &model_path.to_string_lossy());
        info!("llama-server args: {:?}", server_args);
        let stderr_path = log_dir.join("llama-server-stderr.log");
        let server_proc =
            LlamaProcess::spawn(&ws.server_bin, &server_args, "llama-server", Some(&stderr_path))
                .map_err(|e| {
                    error!("Error al lanzar llama-server: {}", e);
                    WorkerError::Other(format!("No se pudo iniciar llama-server: {}", e))
                })?;
        *state.server_process.blocking_lock() = Some(server_proc);
    }

    let modo = match role {
        WorkerRole::SuperNode => "SUPER_NODE (GPU/CPU)",
        WorkerRole::GpuProvider => "GPU_PROVIDER (RPC slave)",
        WorkerRole::Consumer => "CONSUMER (API only)",
    };
    info!(
        "Worker Node listo — modo {}, modelo: {}",
        modo,
        if role == WorkerRole::SuperNode { &model_config.name } else { "N/A" },
    );

    if role == WorkerRole::SuperNode || role == WorkerRole::GpuProvider {
        rt.spawn(monitor_children(state.clone()));
    }
    if role == WorkerRole::SuperNode {
        let model_path = ws.root.join(format!("{}.gguf", model_config.name));
        rt.spawn(handle_slave_updates(
            state.clone(),
            slaves_rx,
            ws.server_bin,
            hw.clone(),
            model_path,
        ));
    }
    if role == WorkerRole::SuperNode {
        info!("SUPER_NODE abriendo túnel de inferencia a {}:{}", ORACLE_HOST, ORACLE_PORT);
        rt.spawn(tunnel::run_reverse_tunnel(
            format!("{}:{}", ORACLE_HOST, ORACLE_PORT),
            format!("127.0.0.1:{}", LLAMA_PORT),
            shutdown.clone(),
            stats.clone(),
        ));
    }
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
    if role == WorkerRole::Consumer {
        info!("CONSUMER: conectado al relay para uso de API (sin túneles locales)");
    }

    rt.block_on(async {
        while !shutdown.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    info!("Cerrando Worker Node...");
    shutdown.store(true, Ordering::SeqCst);
    state.rpc_process.blocking_lock().take();
    state.server_process.blocking_lock().take();

    info!("Worker Node v0.5.1 finalizado.");
    Ok(())
}

// ─── Background Tasks ──────────────────────────────────────────────────────────

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

async fn handle_slave_updates(
    state: Arc<AppState>,
    mut slaves_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    server_bin: std::path::PathBuf,
    hardware: HardwareInfo,
    model_path: std::path::PathBuf,
) {
    while let Some(slaves) = slaves_rx.recv().await {
        info!("Recibida actualización de esclavos RPC: {slaves}");

        let mut guard = state.server_process.lock().await;
        drop(guard.take());

        let local_rpc = format!("127.0.0.1:{RPC_PORT}");
        let rpc_chain = if slaves.is_empty() {
            local_rpc
        } else {
            format!("{local_rpc},{slaves}")
        };

        let args = vec![
            "--rpc".to_string(),
            rpc_chain,
            "-m".to_string(),
            model_path.to_string_lossy().to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            LLAMA_PORT.to_string(),
            "--threads".to_string(),
            hardware.used_threads.to_string(),
            "-ngl".to_string(),
            "999".to_string(),
            "--parallel".to_string(),
            "2".to_string(),
            "--cont-batching".to_string(),
            "--no-kv-offload".to_string(),
        ];
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

// ─── Entry Point ───────────────────────────────────────────────────────────────

fn main() {
    if let Err(e) = check_single_instance() {
        show_error_box(&format!("Worker Node — Error fatal\n\n{}", e));
        return;
    }

    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();

            let tray_menu = build_tray_menu(app.handle())?;
            let _tray = TrayIconBuilder::new()
                .menu(&tray_menu)
                .tooltip("Enjambre Worker Node")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("panel") {
                            center_bottom_right(&window);
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(window) = tray.app_handle().get_webview_window("panel") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                center_bottom_right(&window);
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            start_worker_background(app_handle);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_panel_status, toggle_panel])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
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
