use anyhow::{bail, Context};
use async_compatibility_layer::logging::{setup_backtrace, setup_logging};
use builder::permissioned::init_node;
use clap::Parser;
use cld::ClDuration;
use es_version::SEQUENCER_VERSION;
use ethers::types::Address;
use hotshot::types::{BLSPubKey, SignatureKey};
use hotshot_types::data::ViewNumber;
use hotshot_types::light_client::StateSignKey;
use hotshot_types::signature_key::BLSPrivKey;
use hotshot_types::traits::metrics::NoMetrics;
use hotshot_types::traits::node_implementation::ConsensusTime;
use sequencer::persistence::no_storage::NoStorage;
use sequencer::{BuilderParams, L1Params, NetworkParams};
use snafu::Snafu;
use std::net::ToSocketAddrs;
use std::num::NonZeroUsize;
use std::{collections::HashMap, path::PathBuf, str::FromStr, time::Duration};
use url::Url;

#[derive(Parser, Clone, Debug)]
pub struct PermissionedBuilderOptions {
    /// Unique identifier for this instance of the sequencer network.
    #[clap(long, env = "ESPRESSO_SEQUENCER_CHAIN_ID", default_value = "0")]
    pub chain_id: u16,

    /// URL of the HotShot orchestrator.
    #[clap(
        short,
        long,
        env = "ESPRESSO_SEQUENCER_ORCHESTRATOR_URL",
        default_value = "http://localhost:8080"
    )]
    pub orchestrator_url: Url,

    /// The socket address of the HotShot CDN's main entry point (the marshal)
    /// in `IP:port` form
    #[clap(
        short,
        long,
        env = "ESPRESSO_SEQUENCER_CDN_ENDPOINT",
        default_value = "127.0.0.1:8081"
    )]
    pub cdn_endpoint: String,

    /// The address to bind to for Libp2p (in `host:port` form)
    #[clap(
        short,
        long,
        env = "ESPRESSO_SEQUENCER_LIBP2P_BIND_ADDRESS",
        default_value = "0.0.0.0:1769"
    )]
    pub libp2p_bind_address: String,

    /// The address we advertise to other nodes as being a Libp2p endpoint.
    /// Should be supplied in `host:port` form.
    #[clap(
        short,
        long,
        env = "ESPRESSO_SEQUENCER_LIBP2P_ADVERTISE_ADDRESS",
        default_value = "localhost:1769"
    )]
    pub libp2p_advertise_address: String,

    /// URL of the Light Client State Relay Server
    #[clap(
        short,
        long,
        env = "ESPRESSO_STATE_RELAY_SERVER_URL",
        default_value = "http://localhost:8083"
    )]
    pub state_relay_server_url: Url,

    /// The amount of time to wait between each request to the HotShot
    /// consensus or DA web servers during polling.
    #[clap(
        short,
        long,
        env = "ESPRESSO_SEQUENCER_WEBSERVER_POLL_INTERVAL",
        default_value = "100ms",
        value_parser = parse_duration
    )]
    pub webserver_poll_interval: Duration,

    /// Path to file containing private keys.
    ///
    /// The file should follow the .env format, with two keys:
    /// * ESPRESSO_BUILDER_PRIVATE_STAKING_KEY
    /// * ESPRESSO_BUILDER_PRIVATE_STATE_KEY
    ///
    /// Appropriate key files can be generated with the `keygen` utility program.
    #[clap(long, name = "KEY_FILE", env = "ESPRESSO_BUILDER_KEY_FILE")]
    pub key_file: Option<PathBuf>,

    /// Private staking key.
    ///
    /// This can be used as an alternative to KEY_FILE.
    #[clap(
        long,
        env = "ESPRESSO_BUILDER_PRIVATE_STAKING_KEY",
        conflicts_with = "key_file"
    )]
    pub private_staking_key: Option<BLSPrivKey>,

    /// Private state signing key.
    ///
    /// This can be used as an alternative to KEY_FILE.
    #[clap(
        long,
        env = "ESPRESSO_BUILDER_PRIVATE_STATE_KEY",
        conflicts_with = "key_file"
    )]
    pub private_state_key: Option<StateSignKey>,

    /// Mnemonic phrase for builder account.
    ///
    /// This is the address fees will be charged to.
    /// It must be funded with ETH in the Espresso fee ledger
    #[clap(long, env = "ESPRESSO_BUILDER_ETH_MNEMONIC")]
    pub eth_mnemonic: String,

    /// Index of a funded account derived from eth-mnemonic.
    #[clap(long, env = "ESPRESSO_BUILDER_ETH_ACCOUNT_INDEX", default_value = "8")]
    pub eth_account_index: u32,

    /// Url we will use for RPC communication with L1.
    #[clap(long, env = "ESPRESSO_BUILDER_L1_PROVIDER")]
    pub l1_provider_url: Url,

    /// Peer nodes use to fetch missing state
    #[clap(long, env = "ESPRESSO_SEQUENCER_STATE_PEERS", value_delimiter = ',')]
    pub state_peers: Vec<Url>,

    /// Port to run the builder server on.
    #[clap(short, long, env = "BUILDER_SERVER_PORT")]
    pub port: u16,

    /// Port to run the builder server on.
    #[clap(short, long, env = "BUILDER_ADDRESS")]
    pub address: Address,

    /// Bootstrapping View number
    #[clap(short, long, env = "BUILDER_BOOTSTRAPPED_VIEW")]
    pub view_number: u64,

    /// BUILDER CHANNEL CAPACITY
    #[clap(long, env = "BUILDER_CHANNEL_CAPACITY")]
    pub channel_capacity: NonZeroUsize,

    /// Url a builder can use to stream hotshot events
    #[clap(long, env = "ESPRESSO_SEQUENCER_HOTSHOT_EVENTS_PROVIDER")]
    pub hotshot_events_streaming_server_url: Url,
}

#[derive(Clone, Debug, Snafu)]
pub struct ParseDurationError {
    reason: String,
}

pub fn parse_duration(s: &str) -> Result<Duration, ParseDurationError> {
    ClDuration::from_str(s)
        .map(Duration::from)
        .map_err(|err| ParseDurationError {
            reason: err.to_string(),
        })
}
impl PermissionedBuilderOptions {
    pub fn private_keys(&self) -> anyhow::Result<(BLSPrivKey, StateSignKey)> {
        if let Some(path) = &self.key_file {
            let vars = dotenvy::from_path_iter(path)?.collect::<Result<HashMap<_, _>, _>>()?;
            let staking = vars
                .get("ESPRESSO_BUILDER_PRIVATE_STAKING_KEY")
                .context("key file missing ESPRESSO_BUILDER_PRIVATE_STAKING_KEY")?
                .parse()?;
            let state = vars
                .get("ESPRESSO_BUILDER_PRIVATE_STATE_KEY")
                .context("key file missing ESPRESSO_BUILDER_PRIVATE_STATE_KEY")?
                .parse()?;
            Ok((staking, state))
        } else if let (Some(staking), Some(state)) = (
            self.private_staking_key.clone(),
            self.private_state_key.clone(),
        ) {
            Ok((staking, state))
        } else {
            bail!("neither key file nor full set of private keys was provided")
        }
    }
}
#[async_std::main]
async fn main() -> anyhow::Result<()> {
    setup_logging();
    setup_backtrace();

    let opt = PermissionedBuilderOptions::parse();

    let (private_staking_key, private_state_key) = opt.private_keys()?;

    let l1_params = L1Params {
        url: opt.l1_provider_url,
    };

    let builder_params = BuilderParams {
        mnemonic: opt.eth_mnemonic,
        prefunded_accounts: vec![],
        eth_account_index: opt.eth_account_index,
    };

    // get from the private key
    let builder_pub_key = BLSPubKey::from_private(&private_staking_key);

    // Parse supplied Libp2p addresses to their socket form
    let libp2p_advertise_address = opt
        .libp2p_advertise_address
        .to_socket_addrs()?
        .next()
        .ok_or(anyhow::anyhow!("Failed to resolve Libp2p advertise address"))?;
    let libp2p_bind_address = opt
        .libp2p_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or(anyhow::anyhow!("Failed to resolve Libp2p bind address"))?;

    let network_params = NetworkParams {
        cdn_endpoint: opt.cdn_endpoint,
        libp2p_advertise_address,
        libp2p_bind_address,
        orchestrator_url: opt.orchestrator_url,
        state_relay_server_url: opt.state_relay_server_url,
        private_staking_key: private_staking_key.clone(),
        private_state_key,
        state_peers: opt.state_peers,
    };

    let sequencer_version = SEQUENCER_VERSION;

    let builder_server_url: Url = format!("http://0.0.0.0:{}", opt.port).parse().unwrap();

    let bootstrapped_view = ViewNumber::new(opt.view_number);

    // it will internally spawn the builder web server
    let ctx = init_node(
        network_params,
        &NoMetrics,
        builder_params,
        l1_params,
        builder_server_url.clone(),
        builder_pub_key,
        private_staking_key,
        bootstrapped_view,
        opt.channel_capacity,
        sequencer_version,
        NoStorage,
    )
    .await?;

    // Start doing consensus.
    ctx.start_consensus().await;

    Ok(())
}
