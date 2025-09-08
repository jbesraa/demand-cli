use super::{downstream::Downstream, task_manager::TaskManager};
use crate::{
    proxy_state::{DownstreamType, ProxyState},
    translator::error::Error,
};
use roles_logic_sv2::utils::Mutex;
use std::sync::Arc;
use sv1_api::{client_to_server::Submit, json_rpc};
use tokio::sync::mpsc;
use tokio::task;
use tracing::{error, info, warn};

pub async fn start_receive_downstream(
    task_manager: Arc<Mutex<TaskManager>>,
    downstream: Arc<Mutex<Downstream>>,
    mut recv_from_down: mpsc::Receiver<String>,
    connection_id: u32,
) -> Result<(), Error<'static>> {
    info!("Starting sv1 downstream reader {}", connection_id);
    let task_manager_clone = task_manager.clone();
    let handle = task::spawn(async move {
        while let Some(incoming) = recv_from_down.recv().await {
            let incoming: Result<json_rpc::Message, _> = serde_json::from_str(&incoming);
            if let Ok(incoming) = incoming {
                // if message is Submit Shares update difficulty management
                if let sv1_api::Message::StandardRequest(standard_req) = incoming.clone() {
                    info!(
                        "Received sv1 msg from downstream {}: {:?}",
                        connection_id, standard_req
                    );
                    if let Ok(Submit { .. }) = standard_req.try_into() {
                        if let Err(e) = Downstream::save_share(downstream.clone()) {
                            error!("Failed to save share: {}", e);
                            break;
                        }
                    }
                }

                info!("Handle incoming sv1 msg from downstream {}", connection_id);
                if let Err(error) =
                    Downstream::handle_incoming_sv1(downstream.clone(), incoming).await
                {
                    error!(
                        "Failed to handle incoming sv1 msg from downstream {}: {:?}",
                        connection_id, error
                    );
                    ProxyState::update_downstream_state(DownstreamType::TranslatorDownstream);
                };
            } else {
                // Message received could not be converted to rpc message
                error!(
                    "{}",
                    Error::V1Protocol(Box::new(sv1_api::error::Error::InvalidJsonRpcMessageKind))
                );
                return;
            }
        }
        if let Ok(stats_sender) = downstream.safe_lock(|d| d.stats_sender.clone()) {
            stats_sender.remove_stats(connection_id);
        }
        // No message to receive
        warn!(
            "Downstream: Shutting down sv1 downstream reader {}",
            connection_id
        );

        if let Err(e) = Downstream::remove_downstream_hashrate_from_channel(&downstream) {
            error!("Failed to remove downstream hashrate from channel: {}", e)
        };
        if task_manager_clone
            .safe_lock(|tm| tm.abort_tasks_for_connection_id(connection_id))
            .is_err()
        {
            error!("TaskManager mutex poisoned")
        };
    });
    TaskManager::add_receive_downstream(task_manager, handle.into(), connection_id)
        .await
        .map_err(|_| Error::TranslatorTaskManagerFailed)
}
