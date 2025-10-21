use sv1_api::{
    client_to_server, json_rpc,
    server_to_client::{self, Notify},
    utils::{Extranonce, HexU32Be},
    IsServer,
};
use tracing::{error, info, warn};

use crate::{
    api::stats::StatsSender,
    monitor::worker_activity::{WorkerActivity, WorkerActivityType},
};

use super::downstream::RecentJobs;

struct IntermediateDownstream {
    ///// List of authorized Downstream Mining Devices.
    pub(super) connection_id: u32,
    pub(super) authorized_names: Vec<String>,
    ///// `extranonce1` to be sent to the Downstream in the SV1 `mining.subscribe` message response.
    extranonce1: Vec<u8>,
    ////extranonce2_size: usize,
    ///// Version rolling mask bits
    pub(super) version_rolling_mask: Option<HexU32Be>,
    ///// Minimum version rolling mask bits size
    version_rolling_min_bit: Option<HexU32Be>,
    ///// Sends a SV1 `mining.submit` message received from the Downstream role to the `Bridge` for
    ///// translation into a SV2 `SubmitSharesExtended`.
    //tx_sv1_bridge: Sender<DownstreamMessages>,
    //tx_outgoing: Sender<json_rpc::Message>,
    extranonce2_len: usize,
    //pub(super) difficulty_mgmt: DownstreamDifficultyConfig,
    //pub(super) upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
    //pub last_call_to_update_hr: u128,
    pub(super) stats_sender: StatsSender,
    pub recent_jobs: RecentJobs,
    pub first_job: Notify<'static>,
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
        let mut first_job = self.first_job.clone();
        self.recent_jobs
            .add_job(&mut first_job, self.version_rolling_mask.clone());
        self.first_job = first_job;

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
        self.stats_sender
            .update_device_name(self.connection_id, request.agent_signature.clone());

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
        if self.authorized_names.is_empty() {
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

            true
        } else {
            // when downstream is already authorized we do not want return an ok response otherwise
            // the sv1 proxy could thing that we are saying that downstream produced a valid share.
            warn!("Downstream is trying to authorize again, this should not happen");
            false
        }
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
    fn is_authorized(&self, name: &str) -> bool {
        self.authorized_names.contains(&name.to_string())
    }

    /// Authorizes a Downstream role.
    fn authorize(&mut self, name: &str) {
        self.authorized_names.push(name.to_string());
    }

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
