use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::sleep;
use tracing::{info, warn};
use sysinfo::System;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::hardware::HardwareInfo;

// ─── Roles ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum WorkerRole {
    /// Solo consume la API, no procesa modelos localmente
    Consumer,
    /// Procesa inferencia localmente (requiere GPU con VRAM suficiente)
    SuperNode,
    /// Ofrece su GPU al enjambre como esclavo RPC
    GpuProvider,
}

// ─── ModelConfig ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub role: WorkerRole,
    pub name: String,
    pub url: String,
    pub total_layers: u32,
    pub tunnel_port: u16,
    pub worker_id: String,
}

// ─── ID generation ─────────────────────────────────────────────────────────────

fn generate_worker_id() -> String {
    let hostname = hostname_get();
    let suffix = random_suffix();
    format!("{hostname}-{suffix}")
}

fn hostname_get() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn random_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{:04x}", nanos & 0xFFFF)
}

// ─── Memory helpers ────────────────────────────────────────────────────────────

fn get_free_ram_mb(system: &mut System) -> u64 {
    system.refresh_memory();
    let free_bytes = system.available_memory();
    let free_mb = free_bytes / (1024 * 1024);
    if free_mb > 0 {
        free_mb
    } else {
        let total_mb = system.total_memory() / (1024 * 1024);
        total_mb / 2
    }
}

fn get_free_vram_mb() -> u64 {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"]);
    
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x00000008); // DETACHED_PROCESS
    
    if let Ok(output) = cmd.output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = stdout.lines().next() {
                if let Ok(mb) = line.trim().parse::<u64>() {
                    return mb;
                }
            }
        }
    }
    0
}

// ─── Async download with progress ──────────────────────────────────────────────

async fn download_model(url: &str, dest: &std::path::Path) -> Result<(), String> {
    if dest.exists() {
        let mb = std::fs::metadata(dest)
            .map(|m| m.len() as f64 / 1_048_576.0)
            .unwrap_or(0.0);
        info!("Modelo ya existe ({:.1} MB) — saltando descarga", mb);
        return Ok(());
    }

    info!("⬇  Descargando modelo desde:");
    info!("    {url}");

    let client = reqwest::Client::builder()
        .user_agent("worker-node/0.5.0")
        .timeout(Duration::from_secs(7200))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Error creando cliente HTTP: {e}"))?;

    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Error en descarga: {e}"))?;

    let total = response.content_length().unwrap_or(0);
    let tmp = dest.with_extension("gguf.tmp");

    let mut file = std::fs::File::create(&tmp)
        .map_err(|e| format!("Error creando archivo temporal: {e}"))?;

    let mut downloaded: u64 = 0;
    let start = Instant::now();
    let mut last_log = Instant::now();

    // Descargar por chunks
    use std::io::Write;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Error recibiendo datos: {e}"))?
    {
        file.write_all(&chunk)
            .map_err(|e| format!("Error escribiendo a disco: {e}"))?;
        downloaded += chunk.len() as u64;

        if last_log.elapsed() >= Duration::from_secs(5) {
            if total > 0 {
                let pct = (downloaded as f64 / total as f64) * 100.0;
                let speed = downloaded as f64 / 1_048_576.0 / start.elapsed().as_secs_f64();
                info!(
                    "  Descargando: {:.1}% ({:.0} MB / {:.0} MB) — {:.1} MB/s",
                    pct,
                    downloaded as f64 / 1_048_576.0,
                    total as f64 / 1_048_576.0,
                    speed,
                );
            } else {
                info!(
                    "  Descargando: {:.0} MB",
                    downloaded as f64 / 1_048_576.0,
                );
            }
            last_log = Instant::now();
        }
    }

    // Cerrar y renombrar
    drop(file);
    std::fs::rename(&tmp, dest)
        .map_err(|e| format!("Error moviendo archivo final: {e}"))?;

    let elapsed = start.elapsed();
    info!(
        "✅ Modelo descargado: {:.1} MB en {:.1}s",
        downloaded as f64 / 1_048_576.0,
        elapsed.as_secs_f64(),
    );

    Ok(())
}

// ─── Master Connection ─────────────────────────────────────────────────────────

/// Conexión persistente con el Nodo Maestro.
///
/// 1. Se conecta y registra.
/// 2. Recibe `INIT_MODEL` con nombre, URL y capas.
/// 3. Descarga el modelo si no existe localmente.
/// 4. Envía `config` por el canal `model_ready_tx` para que main.rs continúe.
/// 5. Entra en bucle heartbeat hasta que la conexión se pierda.
/// 6. Si falla, espera 5s y reintenta desde paso 1.
pub async fn start_master_connection(
    master_ip: String,
    master_port: u16,
    hardware: HardwareInfo,
    rpc_port: u16,
    model_ready_tx: tokio::sync::oneshot::Sender<ModelConfig>,
    slaves_tx: tokio::sync::mpsc::UnboundedSender<String>,
    workspace_path: PathBuf,
    credits_tracker: Arc<AtomicU64>,
) {
    let worker_id = generate_worker_id();
    let mut system = System::new();
    let mut ready_tx = Some(model_ready_tx);

    loop {
        info!("Conectando al Maestro {master_ip}:{master_port}...");
        match TcpStream::connect(format!("{master_ip}:{master_port}")).await {
            Ok(mut stream) => {
                // ── Registrar ──────────────────────────────────────────
                let vram_total = hardware.vram_mib.unwrap_or(0);
                let ram_total = (system.total_memory() / (1024 * 1024)).max(1024);
                let has_gpu = if hardware.has_nvidia_gpu { 1 } else { 0 };

                let reg = format!(
                    "REGISTER:{worker_id}:{vram_total}:{ram_total}:{}:{has_gpu}:{rpc_port}\n",
                    hardware.cpu_cores,
                );
                if let Err(e) = stream.write_all(reg.as_bytes()).await {
                    warn!("Error en registro: {e}, reintentando en 5s...");
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }

                // ── Recibir INIT_MODEL ─────────────────────────────────
                let (reader, mut writer) = stream.split();
                let mut buf_reader = BufReader::new(reader);
                let mut response = String::new();
                if buf_reader.read_line(&mut response).await.ok().map_or(true, |n| n == 0) {
                    warn!("No hubo respuesta del Maestro, reintentando...");
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }

                let trimmed = response.trim();
                if !trimmed.starts_with("INIT_MODEL:") {
                    warn!("Respuesta inesperada del Maestro: {trimmed}, reintentando...");
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }

                // Parsear INIT_MODEL:ROLE o INIT_MODEL:ROLE:name:url:layers
                let (role, model_config) = if trimmed.starts_with("INIT_MODEL:SUPER_NODE:") {
                    let rest = &trimmed["INIT_MODEL:SUPER_NODE:".len()..];
                    let parts: Vec<&str> = rest.split(':').collect();
                    if parts.len() < 3 {
                        warn!("INIT_MODEL:SUPER_NODE mal formado: {trimmed}");
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    let name = parts[0].to_string();
                    let url = parts[1..parts.len()-1].join(":");
                    let total_layers = parts.last().and_then(|s| s.parse().ok()).unwrap_or(28);
                    info!("Rol asignado: SUPER_NODE — modelo: {name} ({total_layers} capas)");
                    (WorkerRole::SuperNode, ModelConfig {
                        role: WorkerRole::SuperNode,
                        name,
                        url,
                        total_layers,
                        tunnel_port: 0,
                        worker_id: worker_id.clone(),
                    })
                } else if trimmed.starts_with("INIT_MODEL:CONSUMER") {
                    info!("Rol asignado: CONSUMER — solo consumo de API, sin procesamiento local");
                    (WorkerRole::Consumer, ModelConfig {
                        role: WorkerRole::Consumer,
                        name: String::new(),
                        url: String::new(),
                        total_layers: 0,
                        tunnel_port: 0,
                        worker_id: worker_id.clone(),
                    })
                } else if trimmed.starts_with("INIT_MODEL:GPU_PROVIDER:") {
                    let port_str = trimmed.trim()["INIT_MODEL:GPU_PROVIDER:".len()..].trim();
                    let tunnel_port = port_str.parse::<u16>().unwrap_or(0);
                    info!("Rol asignado: GPU_PROVIDER — túnel puerto {tunnel_port}, RPC server local");
                    (WorkerRole::GpuProvider, ModelConfig {
                        role: WorkerRole::GpuProvider,
                        name: String::new(),
                        url: String::new(),
                        total_layers: 0,
                        tunnel_port,
                        worker_id: worker_id.clone(),
                    })
                } else {
                    warn!("INIT_MODEL con formato desconocido: {trimmed}, reintentando...");
                    sleep(Duration::from_secs(5)).await;
                    continue;
                };

                // ── Descargar modelo si es SUPER_NODE ────────────────
                if role == WorkerRole::SuperNode {
                    let model_path = workspace_path.join(format!("{}.gguf", model_config.name));
                    if let Err(e) = download_model(&model_config.url, &model_path).await {
                        warn!("Error descargando modelo: {e}, reintentando registro...");
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }

                // ── Señalar a main.rs que el modelo/config está listo ─
                if let Some(tx) = ready_tx.take() {
                    if tx.send(model_config.clone()).is_err() {
                        warn!("main.rs no esperaba modelo (canal cerrado)");
                    }
                }

                // ── Bucle Heartbeat ────────────────────────────────────
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                interval.tick().await; // pequeño retardo

                let mut heartbeat_ok = true;
                while heartbeat_ok {
                    interval.tick().await;

                    let vram_free = get_free_vram_mb();
                    let ram_free = get_free_ram_mb(&mut system);

                    let ping = format!("PING:{worker_id}:{vram_free}:{ram_free}\n");
                    match writer.write_all(ping.as_bytes()).await {
                        Ok(()) => {
                            response.clear();
                            match buf_reader.read_line(&mut response).await {
                                Ok(0) | Err(_) => {
                                    warn!("Heartbeat: conexión perdida con Maestro");
                                    heartbeat_ok = false;
                                }
                                Ok(_) => {
                                    let first = response.trim().to_string();
                                    if first.starts_with("PONG") {
                                        // Extraer créditos si están presentes: PONG:CREDITS=123
                                        if let Some(creds_str) = first.strip_prefix("PONG:CREDITS=") {
                                            if let Ok(creds) = creds_str.parse::<u64>() {
                                                credits_tracker.store(creds, Ordering::Relaxed);
                                            }
                                        }
                                        // Posible UPDATE_SLAVES en línea siguiente
                                        let mut extra = String::new();
                                        match tokio::time::timeout(
                                            Duration::from_millis(100),
                                            buf_reader.read_line(&mut extra),
                                        ).await {
                                            Ok(Ok(n)) if n > 0 => {
                                                let extra_trimmed = extra.trim();
                                                if let Some(slaves) = extra_trimmed.strip_prefix("UPDATE_SLAVES:") {
                                                    info!("Topología de esclavos actualizada: {slaves}");
                                                    let _ = slaves_tx.send(slaves.to_string());
                                                } else {
                                                    warn!("Heartbeat: línea extra inesperada: {extra_trimmed}");
                                                }
                                            }
                                            _ => {} // No hay más datos
                                        }
                                     } else if let Some(slaves) = first.strip_prefix("PONG:UPDATE_SLAVES:") {
                                        info!("Topología de esclavos actualizada: {slaves}");
                                        let _ = slaves_tx.send(slaves.to_string());
                                    } else if let Some(slaves) = first.strip_prefix("UPDATE_SLAVES:") {
                                        info!("Topología de esclavos actualizada: {slaves}");
                                        let _ = slaves_tx.send(slaves.to_string());
                                    } else {
                                        warn!("Heartbeat: respuesta inesperada: {first}");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Heartbeat: error enviando PING: {e}");
                            heartbeat_ok = false;
                        }
                    }
                }
                info!("Reconectando al Maestro en 5s...");
            }
            Err(e) => {
                warn!("No se pudo conectar al Maestro: {e}, reintentando en 5s...");
            }
        }

        // Resetear señal en reconexión (el canal ya se consumió, no re-enviamos)
        sleep(Duration::from_secs(5)).await;
    }
}
