use std::sync::RwLock;
use sv1_api::{
    client_to_server, json_rpc,
    server_to_client::{self, Notify},
    utils::{Extranonce, HexU32Be},
    IsServer,
};
use tracing::{error, info};

use crate::monitor::worker_activity::{WorkerActivity, WorkerActivityType};

use super::{downstream::RecentJobs, DownstreamMessages};

pub struct IntermediateDownstream_ {
    // sender: tokio::sync::broadcast::Sender<String>,
    // receiver: tokio::sync::broadcast::Receiver<String>,
}
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StratumMessage {
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value, // This can hold any type of data as the params can vary
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigureParams {
    pub version_rolling: Vec<String>,
    pub version_rolling_mask: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SubscribeParams {
    pub subscription: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AuthorizeParams {
    pub user: String,
    pub password: String,
}


use serde_json::Error as SerdeError;

impl IntermediateDownstream_ {
    pub async fn new(
        _sender: tokio::sync::broadcast::Sender<String>,
        mut receiver: tokio::sync::broadcast::Receiver<String>,
        _tx_sv1_submit: tokio::sync::mpsc::Sender<DownstreamMessages>,
    ) {
        while let Ok(incoming) = receiver.recv().await {
            info!("Received {} from downstream", incoming);
            let message: Result<StratumMessage, SerdeError> = serde_json::from_str(&incoming);
            match message {
                Ok(msg) => {
                    match msg.method.as_str() {
                        "mining.configure" => {
                            let params: Result<ConfigureParams, SerdeError> = serde_json::from_value(msg.params);
                            match params {
                                Ok(configure) => {
                                    info!("Received mining.configure with params: {:?}", configure);
                                }
                                Err(err) => {
                                    error!("Failed to deserialize mining.configure params: {}", err);
                                }
                            }
                        }
                        "mining.subscribe" => {
                            let params: Result<SubscribeParams, SerdeError> = serde_json::from_value(msg.params);
                            match params {
                                Ok(subscribe) => {
                                    info!("Received mining.subscribe with params: {:?}", subscribe);
                                }
                                Err(err) => {
                                    error!("Failed to deserialize mining.subscribe params: {}", err);
                                }
                            }
                        }
                        "mining.authorize" => {
                            let params: Result<AuthorizeParams, SerdeError> = serde_json::from_value(msg.params);
                            match params {
                                Ok(authorize) => {
                                    info!("Received mining.authorize with params: {:?}", authorize);
                                }
                                Err(err) => {
                                    error!("Failed to deserialize mining.authorize params: {}", err);
                                }
                            }
                        }
                        _ => {
                            error!("Unknown method: {}", msg.method);
                        }
                    }
                }
                Err(err) => {
                    error!("Failed to deserialize message: {}", err);
                }
            }
        }
    }
}

pub struct IntermediateDownstream {
    extranonce1: Vec<u8>,
    pub(super) version_rolling_mask: Option<HexU32Be>,
    version_rolling_min_bit: Option<HexU32Be>,
    extranonce2_len: usize,
    pub recent_jobs: RwLock<RecentJobs>,
    pub first_job: RwLock<Notify<'static>>,
    pub user_agent: std::cell::RefCell<String>, // RefCell is used here because `handle_subscribe` and `handle_authorize` take &self not &mut self and we need to mutate user_agent
}

/// Implements `IsServer` for `Downstream` to handle the SV1 messages.
impl IsServer<'static> for IntermediateDownstream {
    /// Handle the incoming `mining.configure` message which is received after a Downstream role is
    /// subscribed and authorized. Contains the version rolling mask parameters.
    fn handle_configure(
        &mut self,
        request: &client_to_server::Configure,
    ) -> (Option<server_to_client::VersionRollingParams>, Option<bool>) {
        info!("Down: Handling mining.configure: {:?}", &request);
        let (version_rolling_mask, version_rolling_min_bit_count) =
            crate::shared::utils::sv1_rolling(request);
        self.version_rolling_mask = Some(version_rolling_mask.clone());
        self.version_rolling_min_bit = Some(version_rolling_min_bit_count.clone());
        (
            Some(server_to_client::VersionRollingParams::new(
                    version_rolling_mask,version_rolling_min_bit_count
            ).expect("Version mask invalid, automatic version mask selection not supported, please change it in carte::downstream_sv1::mod.rs")),
            Some(false),
        )
    }

    /// Handle the response to a `mining.subscribe` message received from the client.
    /// The subscription messages are erroneous and just used to conform the SV1 protocol spec.
    /// Because no one unsubscribed in practice, they just unplug their machine.
    fn handle_subscribe(&self, request: &client_to_server::Subscribe) -> Vec<(String, String)> {
        info!("Down: Handling mining.subscribe: {:?}", &request);
        let set_difficulty_sub = (
            "mining.set_difficulty".to_string(),
            super::new_subscription_id(),
        );
        let notify_sub = (
            "mining.notify".to_string(),
            "ae6812eb4cd7735a302a8a9dd95cf71f".to_string(),
        );
        self.user_agent.replace(request.agent_signature.clone());
        vec![set_difficulty_sub, notify_sub]
    }

    fn handle_authorize(&self, request: &client_to_server::Authorize) -> bool {
        let user_agent = self.user_agent.borrow().clone();
        let worker_activity = WorkerActivity::new(
            user_agent,
            request.name.clone(),
            WorkerActivityType::Connected,
        );

        tokio::spawn(async move {
            if let Err(e) = worker_activity
                .monitor_api()
                .send_worker_activity(worker_activity)
                .await
            {
                error!("Failed to send worker activity: {}", e);
            }
        });

        let mut first_job = self.first_job.write().unwrap();
        let mut recent_jobs = self.recent_jobs.write().unwrap();
        recent_jobs.add_job(&mut first_job, self.version_rolling_mask.clone());
        // *first_job = first_job;

        true
    }

    /// When miner find the job which meets requested difficulty, it can submit share to the server.
    /// Only [Submit](client_to_server::Submit) requests for authorized user names can be submitted.
    fn handle_submit(&self, _request: &client_to_server::Submit<'static>) -> bool {
        unimplemented!();
        // false
    }

    /// Indicates to the server that the client supports the mining.set_extranonce method.
    fn handle_extranonce_subscribe(&self) {}

    /// Checks if a Downstream role is authorized.
    fn is_authorized(&self, _name: &str) -> bool {
        true
    }

    /// Authorizes a Downstream role.
    fn authorize(&mut self, _name: &str) {}

    /// Sets the `extranonce1` field sent in the SV1 `mining.notify` message to the value specified
    /// by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce1(
        &mut self,
        _extranonce1: Option<Extranonce<'static>>,
    ) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().expect("Internal error: this opration can not fail because the Vec<U8> can always be converted into Extranonce")
    }

    /// Returns the `Downstream`'s `extranonce1` value.
    fn extranonce1(&self) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().expect("Internal error: this opration can not fail because the Vec<U8> can always be converted into Extranonce")
    }

    /// Sets the `extranonce2_size` field sent in the SV1 `mining.notify` message to the value
    /// specified by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce2_size(&mut self, _extra_nonce2_size: Option<usize>) -> usize {
        self.extranonce2_len
    }

    /// Returns the `Downstream`'s `extranonce2_size` value.
    fn extranonce2_size(&self) -> usize {
        self.extranonce2_len
    }

    /// Returns the version rolling mask.
    fn version_rolling_mask(&self) -> Option<HexU32Be> {
        self.version_rolling_mask.clone()
    }

    /// Sets the version rolling mask.
    fn set_version_rolling_mask(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_mask = mask;
    }

    /// Sets the minimum version rolling bit.
    fn set_version_rolling_min_bit(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_min_bit = mask
    }

    fn notify(&mut self) -> Result<json_rpc::Message, sv1_api::error::Error> {
        unreachable!()
    }
}
