use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

// ─── Roles ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Consumer,
    SuperNode,
    GpuProvider,
}

impl Role {
    pub fn is_super(&self) -> bool {
        matches!(self, Role::SuperNode)
    }

    pub fn is_consumer(&self) -> bool {
        matches!(self, Role::Consumer)
    }
}

// ─── Configuración del Maestro ─────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub url: String,
    pub total_layers: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MasterConfig {
    pub models: Vec<ModelEntry>,
    #[serde(default = "default_public_ip")]
    pub public_ip: String,
}

fn default_public_ip() -> String {
    "0.0.0.0".to_string()
}

impl MasterConfig {
    pub fn select_model(&self, registry: &WorkerRegistry) -> ModelEntry {
        let workers = registry.get_active_workers();
        let has_super = workers.iter().any(|w| w.role.is_super());
        if has_super && !self.models.is_empty() {
            self.models[0].clone()
        } else {
            self.models[0].clone()
        }
    }
}

pub fn load_config(exe_dir: &Path) -> MasterConfig {
    use std::fs;
    let paths = [
        exe_dir.join("config.toml"),
        exe_dir.join("master-config.toml"),
        Path::new("config.toml").to_path_buf(),
    ];
    for p in &paths {
        if let Ok(content) = fs::read_to_string(p) {
            if let Ok(cfg) = toml::from_str(&content) {
                info!("Configuración cargada desde {}", p.display());
                return cfg;
            }
        }
    }
    info!("Usando configuración por defecto (sin modelos)");
    MasterConfig {
        models: vec![],
        public_ip: default_public_ip(),
    }
}

// ─── Worker Registry ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WorkerNode {
    pub id: String,
    pub role: Role,
    pub addr: String,
    pub rpc_port: u16,
    pub tunnel_port: u16,
    pub vram_mb: u64,
    pub vram_free_mb: u64,
    pub ram_mb: u64,
    pub ram_free_mb: u64,
    pub cpu_cores: usize,
    pub has_gpu: bool,
    pub last_ping: Instant,
    pub credits: u64,
}

const FIRST_TUNNEL_PORT: u16 = 18000;

type CmdTx = tokio::sync::mpsc::UnboundedSender<(String, tokio::sync::oneshot::Sender<Result<String, String>>)>;

pub struct WorkerRegistry {
    workers: Arc<Mutex<HashMap<String, WorkerNode>>>,
    version: Arc<AtomicU64>,
    super_node_id: Arc<Mutex<Option<String>>>,
    last_slave_list: Arc<Mutex<Vec<String>>>,
    tunnel_port_allocator: Arc<Mutex<u16>>,
    credits_file: Option<PathBuf>,
    saved_worker_credits: std::sync::Mutex<HashMap<String, u64>>,
    super_node_cmd_tx: Arc<Mutex<Option<CmdTx>>>,
}

impl WorkerRegistry {
    pub fn new(credits_file: Option<PathBuf>) -> Self {
        let (_pool, saved_credits) = if let Some(ref path) = credits_file {
            Self::load_credits(path)
        } else {
            (1000, HashMap::new())
        };
        Self {
            workers: Arc::new(Mutex::new(HashMap::new())),
            version: Arc::new(AtomicU64::new(0)),
            super_node_id: Arc::new(Mutex::new(None)),
            last_slave_list: Arc::new(Mutex::new(Vec::new())),
            tunnel_port_allocator: Arc::new(Mutex::new(FIRST_TUNNEL_PORT)),
            credits_file,
            saved_worker_credits: std::sync::Mutex::new(saved_credits),
            super_node_cmd_tx: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_super_node_cmd_tx(&self, tx: CmdTx) {
        *self.super_node_cmd_tx.lock().unwrap() = Some(tx);
    }

    pub async fn switch_model(&self, name: &str, url: &str, layers: u32) -> Result<String, String> {
        let tx = {
            self.super_node_cmd_tx.lock().unwrap().clone()
        };
        let tx = tx.ok_or_else(|| "No hay SuperNode conectado".to_string())?;
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let cmd = format!("SWITCH_MODEL:{name}:{url}:{layers}\n");
        tx.send((cmd, resp_tx)).map_err(|_| "SuperNode desconectado".to_string())?;
        resp_rx.await.map_err(|_| "Timeout esperando cambio de modelo".to_string())?
    }

    pub fn allocate_tunnel_port(&self) -> u16 {
        let mut alloc = self.tunnel_port_allocator.lock().unwrap();
        let port = *alloc;
        *alloc = alloc.wrapping_add(1);
        if *alloc < FIRST_TUNNEL_PORT {
            *alloc = FIRST_TUNNEL_PORT;
        }
        port
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    fn bump_version(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn set_super_node(&self, id: &str) {
        let mut sn = self.super_node_id.lock().unwrap();
        *sn = Some(id.to_string());
        self.bump_version();
    }

    pub fn get_super_node_id(&self) -> Option<String> {
        let sn = self.super_node_id.lock().unwrap();
        sn.clone()
    }

    pub fn add_credits_to_super_node(&self, amount: u64, credit_pool: u64) -> u64 {
        let sn_id = {
            let sn = self.super_node_id.lock().unwrap();
            sn.clone()
        };
        if let Some(ref sid) = sn_id {
            let mut map = self.workers.lock().unwrap();
            if let Some(w) = map.get_mut(sid) {
                w.credits += amount;
                let total = w.credits;
                self.bump_version();
                drop(map);
                self.save_credits(credit_pool);
                return total;
            }
        }
        0
    }

    pub fn get_rpc_slaves(&self) -> Vec<WorkerNode> {
        let map = self.workers.lock().unwrap();
        map.values()
            .filter(|w| w.role == Role::GpuProvider)
            .cloned()
            .collect()
    }

    pub fn get_rpc_slave_addrs(&self, _public_ip: &str) -> Vec<String> {
        self.get_rpc_slaves()
            .iter()
            .map(|w| w.addr.clone())
            .collect()
    }

    pub fn check_slave_change(&self, public_ip: &str) -> Option<String> {
        let current = self.get_rpc_slave_addrs(public_ip);
        let mut last = self.last_slave_list.lock().unwrap();
        if current != *last {
            *last = current.clone();
            if current.is_empty() {
                None
            } else {
                Some(format!("UPDATE_SLAVES:{}\n", current.join(",")))
            }
        } else {
            None
        }
    }

    pub fn register(&self, node: WorkerNode) {
        let mut map = self.workers.lock().unwrap();
        map.insert(node.id.clone(), node);
        self.bump_version();
    }

    pub fn unregister(&self, id: &str) {
        {
            let mut map = self.workers.lock().unwrap();
            map.remove(id);
        }
        let was_super = {
            let mut sn = self.super_node_id.lock().unwrap();
            if sn.as_deref() == Some(id) {
                *sn = None;
                true
            } else {
                false
            }
        };
        if was_super {
            let mut cmd_tx = self.super_node_cmd_tx.lock().unwrap();
            cmd_tx.take();
        }
        self.bump_version();
    }

    pub fn update_heartbeat(&self, id: &str, vram_free_mb: u64, ram_free_mb: u64) -> bool {
        let mut map = self.workers.lock().unwrap();
        if let Some(w) = map.get_mut(id) {
            w.vram_free_mb = vram_free_mb;
            w.ram_free_mb = ram_free_mb;
            w.last_ping = Instant::now();
            true
        } else {
            false
        }
    }

    pub fn get_worker_credits(&self, id: &str) -> Option<u64> {
        let map = self.workers.lock().unwrap();
        map.get(id).map(|w| w.credits)
    }

    pub fn get_saved_worker_credit(&self, id: &str) -> u64 {
        self.saved_worker_credits.lock().unwrap().get(id).copied().unwrap_or(0)
    }

    pub fn save_credits(&self, credit_pool: u64) {
        let path = match self.credits_file.as_ref() {
            Some(p) => p.clone(),
            None => return,
        };
        let map = self.workers.lock().unwrap();
        let mut data = serde_json::Map::new();
        data.insert("credit_pool".into(), serde_json::Value::Number(credit_pool.into()));
        let mut workers_map = serde_json::Map::new();
        for (id, w) in map.iter() {
            let mut wdata = serde_json::Map::new();
            wdata.insert("credits".into(), serde_json::Value::Number(w.credits.into()));
            workers_map.insert(id.clone(), serde_json::Value::Object(wdata));
        }
        data.insert("workers".into(), serde_json::Value::Object(workers_map));
        let json = serde_json::Value::Object(data);
        if let Ok(text) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(&path, &text);
        }
    }

    pub fn load_credits(path: &Path) -> (u64, HashMap<String, u64>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return (1000, HashMap::new()),
        };
        let json: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return (1000, HashMap::new()),
        };
        let pool = json["credit_pool"].as_u64().unwrap_or(1000);
        let mut workers = HashMap::new();
        if let Some(wmap) = json["workers"].as_object() {
            for (id, wdata) in wmap {
                if let Some(creds) = wdata["credits"].as_u64() {
                    workers.insert(id.clone(), creds);
                }
            }
        }
        (pool, workers)
    }

    pub fn get_active_workers(&self) -> Vec<WorkerNode> {
        let map = self.workers.lock().unwrap();
        map.values().cloned().collect()
    }

    pub fn get_worker_count(&self) -> usize {
        let map = self.workers.lock().unwrap();
        map.len()
    }
}

// ─── Heartbeat Checker ─────────────────────────────────────────────────────────

pub fn spawn_heartbeat_checker(registry: Arc<WorkerRegistry>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let mut to_remove = Vec::new();
            {
                let workers = registry.get_active_workers();
                let now = Instant::now();
                for w in &workers {
                    if now.duration_since(w.last_ping) > Duration::from_secs(120) {
                        warn!("Worker {} sin heartbeat >120s, eliminando", w.id);
                        to_remove.push(w.id.clone());
                    }
                }
            }
            for id in to_remove {
                registry.unregister(&id);
            }
        }
    });
}

// ─── TCP Protocol ──────────────────────────────────────────────────────────────

#[derive(Debug)]
enum MasterCommand {
    Register { id: String, vram_mb: u64, ram_mb: u64, cpu_cores: usize, has_gpu: bool, rpc_port: u16 },
    Ping { id: String, vram_free_mb: u64, ram_free_mb: u64 },
    Infer { model_name: String },
}

fn parse_line(line: &str) -> Option<MasterCommand> {
    let trimmed = line.trim().to_string();
    let mut parts = trimmed.split(':');
    let cmd = parts.next()?;
    match cmd {
        "REGISTER" => {
            let id = parts.next()?;
            let vram_mb: u64 = parts.next()?.parse().ok()?;
            let ram_mb: u64 = parts.next()?.parse().ok()?;
            let cpu_cores: usize = parts.next()?.parse().ok()?;
            let has_gpu = parts.next()? == "1";
            let rpc_port: u16 = parts.next()?.parse().ok()?;
            Some(MasterCommand::Register {
                id: id.to_string(),
                vram_mb,
                ram_mb,
                cpu_cores,
                has_gpu,
                rpc_port,
            })
        }
        "PING" => {
            let id = parts.next()?;
            let vram_free_mb: u64 = parts.next()?.parse().ok()?;
            let ram_free_mb: u64 = parts.next()?.parse().ok()?;
            Some(MasterCommand::Ping {
                id: id.to_string(),
                vram_free_mb,
                ram_free_mb,
            })
        }
        "INFER" => {
            let model_name = parts.next()?;
            Some(MasterCommand::Infer {
                model_name: model_name.to_string(),
            })
        }
        _ => None,
    }
}

// ─── Layer Splitting ───────────────────────────────────────────────────────────

fn calculate_tensor_split(total_layers: u32, workers: &[WorkerNode]) -> Vec<(String, u32)> {
    let gpu_workers: Vec<&WorkerNode> = workers.iter().filter(|w| w.role == Role::GpuProvider || w.role == Role::SuperNode).collect();
    if gpu_workers.is_empty() {
        return vec![("127.0.0.1:8090".to_string(), total_layers)];
    }
    let n = gpu_workers.len() as u32;
    let base = total_layers / n;
    let extra = (total_layers % n) as usize;
    gpu_workers.iter().enumerate().map(|(i, w)| {
        let layers = if i < extra { base + 1 } else { base };
        (w.addr.clone(), layers)
    }).collect()
}

// ─── Connection Handler ────────────────────────────────────────────────────────

async fn handle_connection(
    mut stream: TcpStream,
    peer: String,
    registry: Arc<WorkerRegistry>,
    config: &MasterConfig,
) {
    let (reader, mut writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    let mut this_worker_id: Option<String> = None;

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<(String, tokio::sync::oneshot::Sender<Result<String, String>>)>();

    loop {
        tokio::select! {
            result = buf_reader.read_line(&mut line) => {
                match result {
                    Ok(0) => {
                        if let Some(ref id) = this_worker_id {
                            registry.unregister(id);
                        }
                        info!("Conexión cerrada por {peer}");
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!("Error leyendo de {peer}: {e}");
                        if let Some(ref id) = this_worker_id {
                            registry.unregister(id);
                        }
                        break;
                    }
                }

                let cmd_line = line.trim().to_string();
                line.clear();
                let cmd = parse_line(&cmd_line);
                match cmd {
                    Some(MasterCommand::Register {
                        id, vram_mb, ram_mb, cpu_cores, has_gpu, rpc_port,
                    }) => {
                        const SUPER_NODE_VRAM_THRESHOLD: u64 = 6144;
                        let role = if has_gpu && vram_mb >= SUPER_NODE_VRAM_THRESHOLD {
                            if registry.get_super_node_id().is_none() {
                                registry.set_super_node(&id);
                                Role::SuperNode
                            } else {
                                Role::GpuProvider
                            }
                        } else {
                            Role::Consumer
                        };

                        let tunnel_port = if role.is_super() {
                            0
                        } else {
                            registry.allocate_tunnel_port()
                        };

                        let saved_credits = registry.get_saved_worker_credit(&id);
                        let node = WorkerNode {
                            id: id.clone(),
                            role: role.clone(),
                            addr: format!("{peer}:{}", rpc_port),
                            rpc_port,
                            tunnel_port,
                            vram_mb,
                            vram_free_mb: vram_mb,
                            ram_mb,
                            ram_free_mb: ram_mb,
                            cpu_cores,
                            has_gpu,
                            last_ping: Instant::now(),
                            credits: saved_credits,
                        };
                        registry.register(node);
                        this_worker_id = Some(id.clone());
                        if role == Role::SuperNode {
                            registry.set_super_node_cmd_tx(cmd_tx.clone());
                        }

                        match role {
                            Role::SuperNode => {
                                let model = config.select_model(&registry);
                                let init = format!("INIT_MODEL:SUPER_NODE:{}:{}:{}\n",
                                    model.name, model.url, model.total_layers);
                                let _ = writer.write_all(init.as_bytes()).await;
                                info!("INIT_MODEL:SUPER_NODE enviado a {id}: {} ({} capas)", model.name, model.total_layers);
                            }
                            Role::Consumer => {
                                let _ = writer.write_all(b"INIT_MODEL:CONSUMER\n").await;
                                info!("INIT_MODEL:CONSUMER enviado a {id}");
                            }
                            Role::GpuProvider => {
                                let init = format!("INIT_MODEL:GPU_PROVIDER:{tunnel_port}\n");
                                let _ = writer.write_all(init.as_bytes()).await;
                                info!("INIT_MODEL:GPU_PROVIDER:{tunnel_port} enviado a {id}");
                            }
                        }
                    }
                    Some(MasterCommand::Ping { id, vram_free_mb, ram_free_mb }) => {
                        if registry.update_heartbeat(&id, vram_free_mb, ram_free_mb) {
                            let credits = registry.get_worker_credits(&id).unwrap_or(0);
                            if let Some(update) = registry.check_slave_change(&config.public_ip) {
                                let _ = writer.write_all(format!("PONG:CREDITS={credits}\n{}", update).as_bytes()).await;
                            } else {
                                let _ = writer.write_all(format!("PONG:CREDITS={credits}\n").as_bytes()).await;
                            }
                        } else {
                            let _ = writer.write_all(b"ERROR:worker_not_found\n").await;
                        }
                    }
                    Some(MasterCommand::Infer { model_name }) => {
                        let model = config.models.iter().find(|m| m.name == model_name);
                        let model = match model {
                            Some(m) => m,
                            None => {
                                let available: Vec<&str> = config.models.iter().map(|m| m.name.as_str()).collect();
                                let err = format!("ERROR:model_not_available:{}\n", available.join(","));
                                let _ = writer.write_all(err.as_bytes()).await;
                                continue;
                            }
                        };

                        let workers = registry.get_active_workers();
                        if workers.is_empty() {
                            let _ = writer.write_all(b"ERROR:no_workers_available\n").await;
                            continue;
                        }

                        let split = calculate_tensor_split(model.total_layers, &workers);
                        let rpc_chain: Vec<String> = split.iter().map(|(addr, _)| addr.clone()).collect();
                        let layer_counts: Vec<String> = split.iter().map(|(_, layers)| layers.to_string()).collect();

                        let response = format!("RPC_CHAIN:{}:{}\n", rpc_chain.join(","), layer_counts.join(","));
                        let _ = writer.write_all(response.as_bytes()).await;

                        info!("INFER {}: workers={}, split=[{}]", model_name, workers.len(), layer_counts.join(","));
                    }
                    None => {
                        let trimmed = line.trim();
                        if trimmed == "MODEL_READY" {
                            // Respuesta a cambio de modelo, ignorar (manejado por cmd_rx)
                        } else {
                            let _ = writer.write_all(b"ERROR:invalid_command\n").await;
                        }
                    }
                }
            }
            Some((cmd, response_tx)) = cmd_rx.recv() => {
                if let Err(e) = writer.write_all(cmd.as_bytes()).await {
                    let _ = response_tx.send(Err(format!("Error enviando comando: {e}")));
                    if let Some(ref id) = this_worker_id {
                        registry.unregister(id);
                    }
                    break;
                }
                let mut resp = String::new();
                match buf_reader.read_line(&mut resp).await {
                    Ok(0) | Err(_) => {
                        let _ = response_tx.send(Err("Conexión perdida durante cambio de modelo".to_string()));
                        if let Some(ref id) = this_worker_id {
                            registry.unregister(id);
                        }
                        break;
                    }
                    Ok(_) => {
                        let _ = response_tx.send(Ok(resp.trim().to_string()));
                    }
                }
            }
        }
    }
}

// ─── Server ────────────────────────────────────────────────────────────────────

pub async fn run_server_on_listener(
    listener: TcpListener,
    registry: Arc<WorkerRegistry>,
    config: Arc<MasterConfig>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let peer = addr.to_string();
                let registry = registry.clone();
                let config = config.clone();
                tokio::spawn(async move {
                    handle_connection(stream, peer, registry, &config).await;
                });
            }
            Err(e) => {
                warn!("Error aceptando conexión: {e}");
            }
        }
    }
}
