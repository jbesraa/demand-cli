use crate::monitor::{worker_activity_server_endpoint, MonitorAPI};

#[derive(serde::Serialize, Debug)]
pub enum WorkerActivityType {
    Connected,
    Disconnected,
}

#[derive(serde::Serialize, Debug)]
pub struct WorkerActivity {
    user_agent: String,
    worker_name: String,
    activity: WorkerActivityType,
}

impl WorkerActivity {
    pub fn new(user_agent: String, worker_name: String, activity: WorkerActivityType) -> Self {
        WorkerActivity {
            user_agent,
            worker_name,
            activity,
        }
    }

    pub fn monitor_api(&self) -> MonitorAPI {
        MonitorAPI::new(worker_activity_server_endpoint())
    }
}
