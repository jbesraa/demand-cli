use roles_logic_sv2::utils::Mutex;
use std::sync::Arc;
use tracing::{debug, error};

use crate::{
    monitor::{shares_server_endpoint, MonitorAPI},
    proxy_state::{DownstreamType, ProxyState},
};

#[derive(serde::Serialize, Clone, Debug)]
pub struct ShareInfo {
    worker_name: String,
    difficulty: Option<f32>,
    job_id: i64,
    // if None, the share was accepted
    rejection_reason: Option<RejectionReason>,
    timestamp: u64,
}

impl ShareInfo {
    pub fn new(
        worker_name: String,
        difficulty: Option<f32>,
        job_id: i64,
        rejection_reason: Option<RejectionReason>,
    ) -> Self {
        ShareInfo {
            worker_name,
            difficulty,
            job_id,
            rejection_reason,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SharesMonitor {
    pending_shares: Arc<Mutex<Vec<ShareInfo>>>,
}

impl SharesMonitor {
    pub fn new() -> Self {
        SharesMonitor {
            pending_shares: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Inserts a new share into the pending shares list.
    pub fn insert_share(&self, share: ShareInfo) {
        self.pending_shares
            .safe_lock(|event| {
                event.push(share);
            })
            .unwrap_or_else(|e| {
                error!("Failed to lock pending shares: {:?}", e);
                ProxyState::update_downstream_state(DownstreamType::TranslatorDownstream);
            });
    }

    /// Retrieves the list of pending shares.
    fn get_pending_shares(&self) -> Vec<ShareInfo> {
        self.pending_shares
            .safe_lock(|event| event.clone())
            .unwrap_or_else(|e| {
                error!("Failed to lock pending shares: {:?}", e);
                ProxyState::update_downstream_state(DownstreamType::TranslatorDownstream);
                Vec::new()
            })
    }

    /// Clears the list of pending shares.
    fn clear_pending_shares(&self) {
        self.pending_shares
            .safe_lock(|event| {
                event.clear();
            })
            .unwrap_or_else(|e| {
                error!("Failed to lock pending shares: {:?}", e);
                ProxyState::update_downstream_state(DownstreamType::TranslatorDownstream);
            });
    }

    /// Monitors the pending shares and sends them to the monitoring server in batches.
    pub async fn monitor(&self) {
        let api = MonitorAPI::new(shares_server_endpoint());
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60)); // Check every 60 seconds
        interval.tick().await; // Skip the first tick to avoid unnecessary error log
        loop {
            interval.tick().await;
            let shares_to_send = self.get_pending_shares();
            if !shares_to_send.is_empty() {
                match api.send_shares(shares_to_send.clone()).await {
                    Ok(_) => {
                        debug!("Successfully sent Shares: {:?} to API", &shares_to_send);
                    }
                    Err(err) => {
                        error!("Failed to send shares: {}", err);
                    }
                }
                self.clear_pending_shares(); // Clear after sending
            } else {
                error!("No pending shares to send");
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum RejectionReason {
    JobIdNotFound,
    InvalidShare,
    InvalidJobIdFormat,
    DifficultyMismatch,
}

impl std::fmt::Display for RejectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RejectionReason::JobIdNotFound => write!(f, "Job ID not found"),
            RejectionReason::InvalidShare => write!(f, "Invalid share"),
            RejectionReason::InvalidJobIdFormat => write!(f, "Invalid job ID format"),
            RejectionReason::DifficultyMismatch => write!(f, "Difficulty mismatch"),
        }
    }
}
