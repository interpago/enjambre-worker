use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Json},
    routing::{get, post, delete},
    Router, body::Body, response::Response,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_stream::StreamExt;
use tokio::time::Duration;

use crate::keys::KeyManager;
use crate::master::{Role, WorkerRegistry};

// ─── Telemetry Structures ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct NetworkStatus {
    pub total_vram_mb: u64,
    pub total_ram_mb: u64,
    pub active_workers_count: usize,
    pub has_active_super_node: bool,
    pub registry_version: u64,
    pub credit_pool: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerDetail {
    pub id: String,
    pub addr: String,
    pub role: String,
    pub vram_free_mb: u64,
    pub ram_free_mb: u64,
    pub seconds_since_last_ping: u64,
    pub credits: u64,
}

// ─── Shared State ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ApiState {
    pub model_name: String,
    pub models: Vec<String>,
    /// Dirección del relay TCP en Oracle Cloud (host:puerto).
    pub relay_addr: String,
    pub http_client: reqwest::Client,
    pub registry: Arc<WorkerRegistry>,
    /// Pool global de créditos para el enjambre.
    pub credit_pool: Arc<AtomicU64>,
    /// Gestor de API keys para usuarios.
    pub key_manager: Arc<KeyManager>,
    /// Ruta al binario worker-node.exe para descarga.
    pub worker_bin_path: Option<String>,
    /// Contraseña para acceder al panel de admin.
    pub admin_password: String,
}

// ─── Router ────────────────────────────────────────────────────────────────────

pub fn build_router(state: ApiState) -> Router {
    Router::new()
        .route("/", get(admin_dashboard))
        .route("/admin", get(admin_dashboard))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/api/network-status", get(get_network_status))
        .route("/api/credits/add", post(add_credits))
        .route("/api/keys", get(list_keys))
        .route("/api/keys", post(create_key))
        .route("/api/keys/:key", delete(delete_key))
        .route("/api/keys/:key/credits", post(add_key_credits))
        .route("/api/keys/:key/toggle", post(toggle_key))
        .route("/download", get(download_worker))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(state)
}

// ─── Admin Auth Helper ─────────────────────────────────────────────────────────

const ADMIN_KEY_HEADER: &str = "x-admin-key";

fn require_admin(state: &ApiState, headers: &axum::http::HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    if state.admin_password.is_empty() {
        return Ok(());
    }
    let provided = headers
        .get(ADMIN_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided == state.admin_password {
        return Ok(());
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "Se requiere X-Admin-Key válida para acceder al panel de administración.",
                "type": "unauthorized"
            }
        })),
    ))
}

// ─── Handlers ──────────────────────────────────────────────────────────────────

async fn download_worker(State(state): State<ApiState>) -> Response {
    match &state.worker_bin_path {
        Some(path) => {
            match tokio::fs::read(path).await {
                Ok(data) => {
                    Response::builder()
                        .header(header::CONTENT_TYPE, "application/octet-stream")
                        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"worker-node.exe\"")
                        .body(Body::from(data))
                        .unwrap()
                }
                Err(_) => {
                    (StatusCode::NOT_FOUND, "worker-node.exe no encontrado en el servidor").into_response()
                }
            }
        }
        None => {
            (StatusCode::NOT_FOUND, "worker-node.exe no disponible").into_response()
        }
    }
}

async fn list_models(State(state): State<ApiState>) -> Json<Value> {
    let data: Vec<Value> = state.models.iter().map(|name| {
        json!({
            "id": name,
            "object": "model",
            "created": 0,
            "owned_by": "enjambre",
            "permission": [{"object": "model-permission"}]
        })
    }).collect();
    Json(json!({
        "object": "list",
        "data": data,
    }))
}

async fn chat_completions(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    // Validar API key
    let api_key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string());

    let key_info = match api_key {
        Some(ref k) => state.key_manager.validate_key(k),
        None => None,
    };

    if key_info.is_none() {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": {
                    "message": "API key inválida o sin créditos. Usa header: Authorization: Bearer sk-enj-...",
                    "type": "invalid_api_key"
                }
            })),
        ));
    }

    // Validar model del body
    let body_value: Value = serde_json::from_slice(&body).map_err(|_| {
        (StatusCode::BAD_REQUEST, Json(json!({"error": "JSON inválido"})))
    })?;
    if let Some(requested) = body_value.get("model").and_then(|v| v.as_str()) {
        if requested != state.model_name {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": {
                        "message": format!("Modelo '{requested}' no disponible. Modelo activo: {}. Cambia el modelo desde el admin panel.", state.model_name),
                        "type": "model_not_available",
                        "available_models": state.models,
                    }
                })),
            ));
        }
    }

    // Verificar créditos globales
    let pool = state.credit_pool.load(Ordering::Relaxed);
    if pool == 0 {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({
                "error": {
                    "message": "Créditos insuficientes. Contacta al administrador para recargar.",
                    "type": "insufficient_credits",
                    "credits_remaining": 0,
                }
            })),
        ));
    }

    let body_bytes = body.to_vec();
    let content_length = body_bytes.len();

    // Construir la petición HTTP/1.1 cruda.
    let http_request = format!(
        "\
POST /v1/chat/completions HTTP/1.1\r\n\
Host: {}\r\n\
Content-Type: application/json\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\
\r\n",
        state.relay_addr, content_length,
    );

    let mut stream = tokio::time::timeout(
        Duration::from_secs(300),
        TcpStream::connect(&state.relay_addr),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({
                "error": {
                    "message": format!("Timeout conectando al relay {}", state.relay_addr),
                    "type": "upstream_error"
                }
            })),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": {
                    "message": format!("Error conectando al relay {}: {e}", state.relay_addr),
                    "type": "upstream_error"
                }
            })),
        )
    })?;

    // Escribir la petición HTTP completa
    let mut full_request = http_request.into_bytes();
    full_request.extend_from_slice(&body_bytes);
    stream
        .write_all(&full_request)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": {
                        "message": format!("Error enviando petición al relay: {e}"),
                        "type": "upstream_error"
                    }
                })),
            )
        })?;

    // Leer la respuesta HTTP cruda del relay
    let mut relay_buf: Vec<u8> = Vec::with_capacity(8192);
    loop {
        let mut chunk = vec![0u8; 65536];
        let n = match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": {
                            "message": format!("Error leyendo respuesta del relay: {e}"),
                            "type": "upstream_error"
                        }
                    })),
                ));
            }
        };
        relay_buf.extend_from_slice(&chunk[..n]);

        if relay_buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    // Parsear la status line y los headers
    let header_end = relay_buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": {
                        "message": "Respuesta del relay sin fin de headers",
                        "type": "upstream_error"
                    }
                })),
            )
        })?;

    let header_block = &relay_buf[..header_end];
    let leftover = &relay_buf[header_end + 4..];

    let header_text = String::from_utf8_lossy(header_block);
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().unwrap_or("HTTP/1.1 502 Bad Gateway");
    let mut status_parts = status_line.split_whitespace();
    let _ = status_parts.next();
    let status_code: u16 = status_parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(502);
    let status =
        axum::http::StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY);

    // Si la respuesta fue exitosa, deducir crédito del pool,
    // acreditar al SuperNode, y descontar de la API key
    if status_code >= 200 && status_code < 300 {
        let pool = state.credit_pool.fetch_sub(1, Ordering::Relaxed) - 1;
        let total = state.registry.add_credits_to_super_node(1, pool);
        if let Some(ref k) = key_info {
            state.key_manager.deduct_credit(&k.key);
        }
        tracing::info!("SuperNode créditos: {total} (pool: {pool}) (request exitoso)");
    }

    let mut content_type = "text/event-stream".to_string();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_lowercase();
            let val = v.trim();
            if key == "content-type" {
                content_type = val.to_string();
            }
        }
    }

    // Transmitir el body
    let leftover_vec = leftover.to_vec();
    let leftover_stream = tokio_stream::iter(
        if leftover_vec.is_empty() {
            Vec::new()
        } else {
            vec![Ok::<_, std::io::Error>(leftover_vec)]
        },
    );

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(8);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(Ok(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }
    });
    let reader_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let stream = leftover_stream.chain(reader_stream);
    let body = Body::from_stream(stream.map(|r| r.map_err(|e| anyhow::anyhow!("{e}"))));

    let mut response = body.into_response();
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).unwrap(),
    );

    Ok(response)
}

async fn add_credits(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers)?;
    let req: Value = serde_json::from_slice(&body).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "JSON inválido"})),
        )
    })?;

    let amount = req.get("amount").and_then(|v| v.as_u64()).unwrap_or(0);
    if amount == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "amount debe ser > 0"})),
        ));
    }

    state.credit_pool.fetch_add(amount, Ordering::Relaxed);
    let pool = state.credit_pool.load(Ordering::Relaxed);
    state.registry.save_credits(pool);

    Ok(Json(json!({
        "status": "ok",
        "added": amount,
        "credit_pool": pool,
    })))
}

async fn get_network_status(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers)?;
    let workers = state.registry.get_active_workers();
    let version = state.registry.version();
    let sn_id = state.registry.get_super_node_id();
    let credit_pool = state.credit_pool.load(Ordering::Relaxed);

    let mut total_vram_mb: u64 = 0;
    let mut total_ram_mb: u64 = 0;

    let details: Vec<WorkerDetail> = workers
        .iter()
        .map(|w| {
            total_vram_mb += w.vram_free_mb;
            total_ram_mb += w.ram_free_mb;
            WorkerDetail {
                id: w.id.clone(),
                addr: w.addr.clone(),
                role: match w.role {
                    Role::SuperNode => "SuperNode".to_string(),
                    Role::Consumer => "Consumer".to_string(),
                    Role::GpuProvider => "GpuProvider".to_string(),
                },
                vram_free_mb: w.vram_free_mb,
                ram_free_mb: w.ram_free_mb,
                seconds_since_last_ping: w.last_ping.elapsed().as_secs(),
                credits: w.credits,
            }
        })
        .collect();

    Ok(Json(json!({
        "status": NetworkStatus {
            total_vram_mb,
            total_ram_mb,
            active_workers_count: workers.len(),
            has_active_super_node: sn_id.is_some()
                && workers.iter().any(|w| w.role.is_super()),
            registry_version: version,
            credit_pool,
        },
        "workers": details,
    })))
}

async fn admin_dashboard() -> impl IntoResponse {
    let html = include_str!("../admin.html");
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html)
}

// ─── Key Management Handlers ─────────────────────────────────────────────────

async fn list_keys(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers)?;
    let keys = state.key_manager.list_keys();
    Ok(Json(json!({ "keys": keys })))
}

async fn create_key(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers)?;
    let req: Value = serde_json::from_slice(&body).map_err(|_| {
        (StatusCode::BAD_REQUEST, Json(json!({"error": "JSON inválido"})))
    })?;
    let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("Sin nombre").to_string();
    let credits = req.get("credits").and_then(|v| v.as_u64()).unwrap_or(100);
    let key = state.key_manager.create_key(name, credits);
    Ok(Json(json!({ "status": "ok", "key": key })))
}

async fn delete_key(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers)?;
    let deleted = state.key_manager.delete_key(&key);
    Ok(Json(json!({ "status": if deleted { "ok" } else { "not_found" } })))
}

async fn add_key_credits(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(key): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers)?;
    let req: Value = serde_json::from_slice(&body).map_err(|_| {
        (StatusCode::BAD_REQUEST, Json(json!({"error": "JSON inválido"})))
    })?;
    let amount = req.get("amount").and_then(|v| v.as_u64()).unwrap_or(0);
    if amount == 0 {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "amount debe ser > 0"}))));
    }
    let ok = state.key_manager.add_credits(&key, amount);
    if ok {
        let entry = state.key_manager.get_key(&key);
        Ok(Json(json!({ "status": "ok", "credits": entry.map(|e| e.credits).unwrap_or(0) })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(json!({"error": "Key no encontrada"}))))
    }
}

async fn toggle_key(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers)?;
    let ok = state.key_manager.toggle_key(&key);
    if ok {
        let entry = state.key_manager.get_key(&key);
        Ok(Json(json!({ "status": "ok", "enabled": entry.map(|e| e.enabled).unwrap_or(false) })))
    } else {
        Ok(Json(json!({ "status": "not_found" })))
    }
}
