use worker_node_core::api;
use worker_node_core::keys::KeyManager;
use worker_node_core::master;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::env;
use tokio::net::TcpListener;
use axum::serve;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    let exe_dir = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| env::current_dir().unwrap());

    let config = Arc::new(master::load_config(&exe_dir));

    let args: Vec<String> = env::args().collect();
    let port = args
        .get(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(7001);

    let worker_addr = format!("0.0.0.0:{port}");
    let http_addr = format!("0.0.0.0:{}", std::env::var("MASTER_HTTP_PORT").unwrap_or_else(|_| "8081".into()));

    let registry = Arc::new(master::WorkerRegistry::new(Some(exe_dir.join("credits.json"))));
    let initial_pool = {
        let (pool, _) = master::WorkerRegistry::load_credits(&exe_dir.join("credits.json"));
        pool
    };
    let tunnel_port: u16 = std::env::var("MASTER_TUNNEL_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7000);
    // El relay TCP corre en Oracle Cloud en la IP pública del Master y en el
    // puerto del túnel (7000 por defecto para el SuperNode).
    let relay_addr = format!("{}:{}", config.public_ip, tunnel_port);
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .expect("Error creando HTTP client");

    tracing::info!("=== Enjambre Nodo Maestro v0.5.0 (Super Node Topology) ===");
    tracing::info!("Workers TCP: {worker_addr}");
    tracing::info!("HTTP API:    {http_addr}");
    tracing::info!("Relay TCP:   {relay_addr}");
    for m in &config.models {
        tracing::info!("Modelo:      {} ({} capas)", m.name, m.total_layers);
    }
    tracing::info!("Túnel SUPER_NODE: puerto {tunnel_port} → relay {relay_addr}");

    // ── Heartbeat checker (fondo) ──────────────────────────────────────
    master::spawn_heartbeat_checker(registry.clone());

    // ── API State ─────────────────────────────────────────────────────
    // El modelo activo se determina dinámicamente según los workers
    let active_model = config.select_model(&registry);
    let all_models: Vec<String> = config.models.iter().map(|m| m.name.clone()).collect();
    let api_state = api::ApiState {
        model_name: active_model.name.clone(),
        models: all_models,
        relay_addr,
        http_client,
        registry: registry.clone(),
        credit_pool: Arc::new(AtomicU64::new(initial_pool)),
        key_manager: Arc::new(KeyManager::new(Some(exe_dir.join("keys.json")))),
        worker_bin_path: Some("/home/ubuntu/worker-node.exe".to_string()),
        admin_password: config.admin_password.clone(),
    };

    let router = api::build_router(api_state);

    // ── Servidores concurrentes ────────────────────────────────────────
    let tcp_listener = TcpListener::bind(&worker_addr)
        .await
        .expect("Error bindeando TCP worker");
    let http_listener = TcpListener::bind(&http_addr)
        .await
        .expect("Error bindeando HTTP API");

    tracing::info!("Servidores listos");

    tokio::select! {
        _ = master::run_server_on_listener(tcp_listener, registry, config) => {
            tracing::error!("Servidor TCP terminó inesperadamente");
        }
        _ = serve(http_listener, router) => {
            tracing::error!("Servidor HTTP terminó inesperadamente");
        }
    }
}
