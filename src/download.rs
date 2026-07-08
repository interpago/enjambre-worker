use std::path::Path;
use std::time::Instant;
use tracing::info;

/// Descarga un modelo GGUF desde `url` a `dest`.
/// Si el archivo ya existe, no hace nada.
/// Usa reqwest blocking (llamada sincrónica desde main).
pub fn ensure_model(url: &str, name: &str, dest: &Path) -> Result<(), anyhow::Error> {
    if dest.exists() {
        let mb = std::fs::metadata(dest)
            .map(|m| m.len() as f64 / 1_048_576.0)
            .unwrap_or(0.0);
        info!("{name} ya existe ({:.1} MB) — saltando descarga", mb);
        return Ok(());
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("worker-node/0.5.0")
        .timeout(std::time::Duration::from_secs(7200))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()?;

    info!("⬇  Descargando {name} desde:");
    info!("    {url}");

    let start = Instant::now();
    let response = client.get(url).send()?;
    let total = response.content_length().unwrap_or(0);
    let tmp = dest.with_extension("gguf.tmp");
    let bytes = response.bytes()?;

    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dest)?;

    let elapsed = start.elapsed();

    if total > 0 {
        let pct = (bytes.len() as f64 / total as f64) * 100.0;
        info!("  {:.1}% — {} bytes descargados", pct, bytes.len());
    }

    info!("✅ Modelo descargado en {:.1}s", elapsed.as_secs_f64());
    Ok(())
}
