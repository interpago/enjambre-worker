use std::sync::atomic::{AtomicU64, Ordering};

const BYTES_PER_TOKEN: u64 = 4;
const TOKENS_PER_CREDIT: u64 = 500;

pub struct WorkerStats {
    pub bytes_to_oracle: AtomicU64,
    pub bytes_to_worker: AtomicU64,
}

impl WorkerStats {
    pub fn new() -> Self {
        Self {
            bytes_to_oracle: AtomicU64::new(0),
            bytes_to_worker: AtomicU64::new(0),
        }
    }

    pub fn add_to_oracle(&self, n: u64) {
        self.bytes_to_oracle.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_to_worker(&self, n: u64) {
        self.bytes_to_worker.fetch_add(n, Ordering::Relaxed);
    }

    pub fn total_bytes(&self) -> u64 {
        self.bytes_to_oracle.load(Ordering::Relaxed)
            + self.bytes_to_worker.load(Ordering::Relaxed)
    }

    pub fn estimated_tokens(&self) -> u64 {
        let t = self.total_bytes() / BYTES_PER_TOKEN;
        if t < 1 { 1 } else { t }
    }

    pub fn credits(&self) -> u64 {
        self.estimated_tokens() / TOKENS_PER_CREDIT
    }
}
