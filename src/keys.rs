use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use serde::{Serialize, Deserialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub key: String,
    pub name: String,
    pub credits: u64,
    pub enabled: bool,
    pub created_at: String,
}

pub struct KeyManager {
    keys: Mutex<HashMap<String, ApiKey>>,
    path: Option<PathBuf>,
}

impl KeyManager {
    pub fn new(path: Option<PathBuf>) -> Self {
        let keys = if let Some(ref p) = path {
            Self::load(p).unwrap_or_default()
        } else {
            HashMap::new()
        };
        KeyManager { keys: Mutex::new(keys), path }
    }

    fn generate_key_string() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let hex = format!("{:016x}", (nanos & 0xFFFFFFFFFFFFFFFF) as u64);
        format!("sk-enj-{}", &hex[..16])
    }

    pub fn create_key(&self, name: String, initial_credits: u64) -> String {
        let key = Self::generate_key_string();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let entry = ApiKey {
            key: key.clone(),
            name,
            credits: initial_credits,
            enabled: true,
            created_at: now.to_string(),
        };
        self.keys.lock().unwrap().insert(key.clone(), entry);
        self.save();
        key
    }

    pub fn validate_key(&self, key: &str) -> Option<ApiKey> {
        let map = self.keys.lock().unwrap();
        map.get(key).filter(|k| k.enabled && k.credits > 0).cloned()
    }

    pub fn get_key(&self, key: &str) -> Option<ApiKey> {
        self.keys.lock().unwrap().get(key).cloned()
    }

    pub fn deduct_credit(&self, key: &str) -> bool {
        let mut map = self.keys.lock().unwrap();
        if let Some(entry) = map.get_mut(key) {
            if entry.credits > 0 {
                entry.credits -= 1;
                drop(map);
                self.save();
                return true;
            }
        }
        false
    }

    pub fn list_keys(&self) -> Vec<ApiKey> {
        let mut list: Vec<ApiKey> = self.keys.lock().unwrap().values().cloned().collect();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        list
    }

    pub fn delete_key(&self, key: &str) -> bool {
        let mut map = self.keys.lock().unwrap();
        let existed = map.remove(key).is_some();
        if existed {
            drop(map);
            self.save();
        }
        existed
    }

    pub fn add_credits(&self, key: &str, amount: u64) -> bool {
        let mut map = self.keys.lock().unwrap();
        if let Some(entry) = map.get_mut(key) {
            entry.credits += amount;
            drop(map);
            self.save();
            return true;
        }
        false
    }

    pub fn toggle_key(&self, key: &str) -> bool {
        let mut map = self.keys.lock().unwrap();
        if let Some(entry) = map.get_mut(key) {
            entry.enabled = !entry.enabled;
            drop(map);
            self.save();
            return true;
        }
        false
    }

    fn load(path: &Path) -> Option<HashMap<String, ApiKey>> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save(&self) {
        let path = match self.path.as_ref() {
            Some(p) => p,
            None => return,
        };
        let map = self.keys.lock().unwrap();
        if let Ok(text) = serde_json::to_string_pretty(&*map) {
            let _ = std::fs::write(path, &text);
        }
    }
}
