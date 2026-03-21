pub struct ClaimRace {
    worker_id: String,
}

impl ClaimRace {
    pub fn new(worker_id: String) -> Self {
        Self { worker_id }
    }

    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }
}
