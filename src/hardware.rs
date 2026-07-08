use std::process::Command;
use tracing::{debug, info};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[derive(Debug, Clone)]
pub struct HardwareInfo {
    pub has_nvidia_gpu: bool,
    pub nvidia_gpu_name: Option<String>,
    /// VRAM total en MiB (via nvidia-smi)
    pub vram_mib: Option<u64>,
    /// Hilos lógicos totales del procesador (SMT/HyperThreading incluidos)
    pub cpu_cores: usize,
    /// Hilos que usaremos = max(1, cpu_cores / 2) para no saturar el PC
    pub used_threads: usize,
}

impl Default for HardwareInfo {
    fn default() -> Self {
        let total = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            has_nvidia_gpu: false,
            nvidia_gpu_name: None,
            vram_mib: None,
            cpu_cores: total,
            used_threads: std::cmp::max(1, total / 2),
        }
    }
}

pub fn detect() -> HardwareInfo {
    let mut info = HardwareInfo::default();

    // ── 1. Detectar GPU via nvidia-smi ─────────────────────────────
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,memory.total", "--format=csv,noheader"]);
    
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x00000008); // DETACHED_PROCESS
    
    if let Ok(output) = cmd.output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = stdout.lines().next() {
                let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 2 {
                    info.has_nvidia_gpu = true;
                    info.nvidia_gpu_name = Some(parts[0].to_string());
                    if let Some(val) = parts[1].split_whitespace().next() {
                        info.vram_mib = val.parse::<u64>().ok();
                    }
                    debug!("GPU NVIDIA: {} — VRAM: {:?} MiB", parts[0], info.vram_mib);
                }
            }
        }
    }

    // ── 2. Fallback: registro de Windows ───────────────────────────
    if !info.has_nvidia_gpu {
        info.has_nvidia_gpu = detect_nvidia_via_registry();
    }

    // ── 3. Mostrar resumen de hardware ─────────────────────────────
    if info.has_nvidia_gpu {
        info!(
            "Hardware: GPU [{}], VRAM={:?} MiB, CPUs totales={}, hilos usados={}",
            info.nvidia_gpu_name.as_deref().unwrap_or("NVIDIA"),
            info.vram_mib,
            info.cpu_cores,
            info.used_threads,
        );
    } else {
        info!(
            "Hardware: CPU-only, CPUs totales={}, hilos usados={}",
            info.cpu_cores, info.used_threads,
        );
    }

    info
}

fn detect_nvidia_via_registry() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(nvidia_key) = hklm.open_subkey(r"SOFTWARE\NVIDIA Corporation\GPU") {
        for subkey_name in nvidia_key.enum_keys().filter_map(Result::ok) {
            if let Ok(subkey) = nvidia_key.open_subkey(&subkey_name) {
                if let Ok(name) = subkey.get_value::<String, _>("Name") {
                    if !name.is_empty() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Construye los argumentos para ggml-rpc-server.
///
/// ggml-rpc-server es un bridge RPC ligero. No carga modelos
/// localmente — solo expone el hardware (CPU/GPU) al nodo maestro
/// que se conecta por RPC. Por eso solo necesita host, puerto e
/// hilos de CPU.
///
/// ## Hilos de CPU
///
/// Usamos `used_threads` (la mitad de los hilos lógicos) para `--threads`.
/// Esto evita saturar el PC del usuario: deja núcleos libres para el SO
/// y otras aplicaciones.
///
/// ## VRAM (nodo maestro → worker)
///
/// El worker expone su VRAM total (`vram_mib`) al registrarse con el
/// servidor central. El nodo maestro usa esa info para decidir cuántas
/// capas enviar al worker. ggml-rpc-server maneja el offloading de
/// capas por solicitud RPC; no necesita flags de -ngl fijos.
pub fn build_rpc_args(info: &HardwareInfo, port: u16) -> Vec<String> {
    vec![
        "--host".to_string(),
        "0.0.0.0".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--threads".to_string(),
        info.used_threads.to_string(),
    ]
}

/// Construye los argumentos para llama-server (OpenAI-compatible HTTP API).
///
/// llama-server carga el modelo localmente y expone `/v1/chat/completions`.
/// Usa `--rpc` para delegar el cómputo a ggml-rpc-server.
pub fn build_llama_server_args(
    info: &HardwareInfo,
    rpc_port: u16,
    http_port: u16,
    model_path: &str,
) -> Vec<String> {
    vec![
        "--rpc".to_string(),
        format!("127.0.0.1:{}", rpc_port),
        "-m".to_string(),
        model_path.to_string(),
        "--host".to_string(),
        "0.0.0.0".to_string(),
        "--port".to_string(),
        http_port.to_string(),
        "--threads".to_string(),
        info.used_threads.to_string(),
        "-ngl".to_string(),
        "999".to_string(),
        "--parallel".to_string(),
        "2".to_string(),
        "--cont-batching".to_string(),
        "--no-kv-offload".to_string(),
    ]
}
