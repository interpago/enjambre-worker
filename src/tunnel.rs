use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use socket2::TcpKeepalive;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, info, warn};

use crate::stats::WorkerStats;

fn set_keepalive(stream: &TcpStream) {
    let sock_ref = socket2::SockRef::from(stream);
    let _ = sock_ref.set_tcp_keepalive(
        &TcpKeepalive::new()
            .with_time(Duration::from_secs(30))
            .with_interval(Duration::from_secs(10)),
    );
}

pub async fn run_reverse_tunnel(
    server_addr: String,
    local_addr: String,
    shutdown: Arc<AtomicBool>,
    stats: Arc<WorkerStats>,
) {
    info!("Túnel reverso iniciado — servidor: {}", server_addr);

    while !shutdown.load(Ordering::SeqCst) {
        info!("Conectando al servidor central {}...", server_addr);

        match TcpStream::connect(&server_addr).await {
            Ok(mut server) => {
                set_keepalive(&server);
                info!("✓ Túnel establecido con Oracle Cloud");

                // Identificarse como worker ante el relay
                let _ = server.write_all(b"E").await;

                // Leer primer byte del relay (se bloquea hasta que llegue API request)
                let mut first_byte = [0u8; 1];
                if let Err(e) = server.read_exact(&mut first_byte).await {
                    warn!("Error leyendo primer byte del relay: {}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }

                // Conectar a llama-server SOLO cuando hay datos que enviar
                let mut local = match TcpStream::connect(&local_addr).await {
                    Ok(stream) => {
                        info!("Conectado a {} para forward", local_addr);
                        stream
                    }
                    Err(e) => {
                        warn!("No se pudo conectar a {}: {}", local_addr, e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };

                // Enviar primer byte y luego copiar bidireccionalmente el resto
                if let Err(e) = local.write_all(&first_byte).await {
                    warn!("Error escribiendo primer byte a {}: {}", local_addr, e);
                    continue;
                }

                match io::copy_bidirectional(&mut server, &mut local).await {
                    Ok((to_oracle, to_worker)) => {
                        stats.add_to_oracle(to_oracle);
                        stats.add_to_worker(to_worker);
                        info!(
                            "Túnel cerrado: {} B → Oracle, {} B → worker",
                            to_oracle, to_worker
                        );
                    }
                    Err(e) => warn!("Túnel interrumpido: {}", e),
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("timed out") || err_str.contains("refused") {
                    warn!("Oracle no responde — reintento en 5s");
                } else {
                    error!("Error de conexión a Oracle: {}", e);
                }
            }
        }

        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    info!("Túnel reverso finalizado");
}


