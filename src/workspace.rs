use std::io::Cursor;
use std::path::PathBuf;
use tracing::{debug, info};

/// ZIP conteniendo el motor llama.cpp (ggml-rpc-server.exe + DLLs).
/// Debe estar en la raíz del proyecto (junto a Cargo.toml) al compilar.
const EMBEDDED_ZIP: &[u8] = include_bytes!("../llama-engine.zip");

/// Gestiona el directorio de trabajo dentro de `%LOCALAPPDATA%\Enjambre`.
/// Extrae el ZIP embebido en memoria y descarga el modelo allí.
pub struct Workspace {
    pub root: PathBuf,
    pub rpc_bin: PathBuf,
    pub server_bin: PathBuf,
    pub model_path: PathBuf,
}

impl Workspace {
    pub fn init() -> Result<Self, anyhow::Error> {
        let appdata = std::env::var("LOCALAPPDATA")
            .map_err(|_| anyhow::anyhow!(
                "Variable de entorno LOCALAPPDATA no definida"
            ))?;
        let root = PathBuf::from(appdata).join("Enjambre");
        std::fs::create_dir_all(&root)?;

        info!("Workspace: {}", root.display());

        Ok(Self {
            rpc_bin: root.join("ggml-rpc-server.exe"),
            server_bin: root.join("llama-server.exe"),
            model_path: root.join("llama3.2-3b-instruct-q4_k_m.gguf"),
            root,
        })
    }

    pub fn extract_llama(&self) -> Result<(), anyhow::Error> {
        if self.rpc_bin.exists() {
            debug!(
                "ggml-rpc-server.exe ya extraído en {} — saltando",
                self.rpc_bin.display()
            );
            return Ok(());
        }

        info!(
            "Extrayendo motor llama.cpp (~52 archivos, {} bytes)...",
            EMBEDDED_ZIP.len()
        );

        let cursor = Cursor::new(EMBEDDED_ZIP);
        let mut archive = zip::ZipArchive::new(cursor)?;
        let count = archive.len();

        for i in 0..count {
            let mut entry = archive.by_index(i)?;
            let entry_name = entry.name().to_string();

            if entry_name.contains("..") {
                debug!("Saltando entrada con path traversal: {}", entry_name);
                continue;
            }

            let outpath = self.root.join(&entry_name);

            if entry.is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut entry, &mut outfile)?;
            }

            debug!("  extraído: {}", entry_name);
        }

        info!("Motor extraído correctamente ({} entradas)", count);
        Ok(())
    }
}
