#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]
#![allow(deprecated)]

/*
0x775c559d9a48ce5a8444c1035c3a8921ab477b8e
0x00000000003b3cc22af3ae1eac0440bcee416b40
 */
// TODO: node url dispacher
mod abi;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn, Instrument};
//Deployer: 0xef344B9eFcc133EB4e7FEfbd73a613E3b2D05e86
// Deployed to: 0x5F416E55fdBbA8CC0D385907C534B57a08710c35
// Transaction hash: 0x2f03f71cc6915fcc17afd71730f1fb6809cef88b102fcb3aa3e3dc60095d1aee
//V2
//	Deployer: 0xef344B9eFcc133EB4e7FEfbd73a613E3b2D05e86
// Deployed to: 0x3fDaA7c06379981c879F7a9617470215f808368F
// Transaction hash: 0x1772c078cc5749dd0dcbebf316280ac06eb41735d501abd8a2a13521db6e16bd
use once_cell::sync::Lazy;

use async_std::sync::Arc;
use ethers::prelude::StreamExt;
use ethers::prelude::{Address, LocalWallet, SignerMiddleware, H160, H256};
use std::collections::HashMap;
use tokio::runtime::Runtime;
use tokio::sync::RwLock;

use async_trait::async_trait;
use ethers::abi::{AbiEncode, ParamType, StateMutability, Token};
use ethers::core::k256::elliptic_curve::consts::U25;
use ethers::prelude::k256::elliptic_curve::consts::U2;
use ethers::signers::Signer;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{
    transaction::eip2930::AccessList, Block, BlockId, BlockNumber, Eip1559TransactionRequest,
    NameOrAddress, Transaction, TxHash, U256, U64,
};
use ethers::types::{Bytes, I256};
use ethers::utils::keccak256;
use ethers_flashbots::{
    BundleRequest, BundleTransaction, FlashbotsMiddleware, FlashbotsMiddlewareError, RelayError,
    SimulatedBundle,
};
use ethers_providers::{Http, Middleware, Provider, ProviderExt, Ws};
use futures::future::err;
use garb_graph_eth::backrun::Backrun;
use garb_graph_eth::mev_path::MevPath;
use garb_graph_eth::single_arb::ArbPath;
use garb_graph_eth::{GraphConfig, Order};
use garb_sync_eth::node_dispatcher::NodeDispatcher;
use garb_sync_eth::{
    EventSource, LiquidityProviderId, LiquidityProviders, PendingPoolUpdateEvent, Pool,
    PoolUpdateEvent, SyncConfig,
};
use rand::Rng;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::Duration;
use url::Url;

static PROVIDERS: Lazy<Vec<LiquidityProviders>> = Lazy::new(|| {
    std::env::var("ETH_PROVIDERS")
        .unwrap_or_else(|_| {
            std::env::args()
                .nth(5)
                .unwrap_or("3,5,6,8".to_string())
        })
        .split(",")
        .map(|i| LiquidityProviders::from(i))
        .collect()
});

static CONTRACT_ADDRESS: Lazy<Address> = Lazy::new(|| {
    Address::from_str(&std::env::var("ETH_CONTRACT_ADDRESS").unwrap_or_else(|_| {
        std::env::args()
            .nth(6)
            .unwrap_or("0xA46356ba716631d87Ab3081635F06136662ae3C0".to_string())
    }))
    .unwrap()
});

static NODE_URL: Lazy<Url> = Lazy::new(|| {
    let url = std::env::var("ETH_NODE_URL").unwrap_or_else(|_| {
        std::env::args()
            .nth(7)
            .unwrap_or("http://65.21.198.115:8545".to_string())
    });
    Url::parse(&url).unwrap()
});

static PRIVATE_KEY: Lazy<String> = Lazy::new(|| std::env::var("ETH_PRIVATE_KEY").unwrap());
static BUNDLE_SIGNER_PRIVATE_KEY: Lazy<String> =
    Lazy::new(|| std::env::var("ETH_BUNDLE_SIGNER_PRIVATE_KEY").unwrap());

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .compact()
        .log_internal_errors(true)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let rt = Runtime::new()?;
    rt.block_on(async_main())?;
    Ok(())
}

pub async fn async_main() -> anyhow::Result<()> {
    let pools = Arc::new(RwLock::new(HashMap::<String, Pool>::new()));
    let (update_q_sender, update_q_receiver) =
        kanal::bounded_async::<Box<dyn EventSource<Event = PoolUpdateEvent>>>(10000);
    let (pending_update_q_sender, pending_update_q_receiver) =
        kanal::bounded_async::<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>(10000);
    // routes holds the routes that pass through an updated pool
    // this will be populated by the graph module when there is an updated pool
    let (routes_sender, mut routes_receiver) = kanal::bounded_async::<Backrun>(1000);
    let (single_routes_sender, mut single_routes_receiver) = kanal::bounded::<Vec<ArbPath>>(1000);
    let (pool_sender, mut pool_receiver) = kanal::bounded_async::<Pool>(10000);
    let (used_pools_shot_tx, used_pools_shot_rx) =
        tokio::sync::oneshot::channel::<Vec<[Arc<RwLock<Pool>>; 2]>>();

    let from_file = std::env::args()
        .nth(1)
        .unwrap_or("true".to_string()).parse::<bool>().unwrap_or(true);
    let sync_config = SyncConfig {
        providers: PROVIDERS.clone(),
        from_file
    };

    let nodes = NodeDispatcher::from_file("nodes").await?;
    garb_sync_eth::start(
        pools.clone(),
        update_q_sender,
        pending_update_q_sender,
        used_pools_shot_rx,
        nodes.clone(),
        sync_config,
    )
    .await
    .unwrap();

    let mut joins = vec![];

    let graph_conifg = GraphConfig {
        from_file,
        save_only: true,
    };
    let graph_routes = routes_sender.clone();
    joins.push(std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();

        rt.block_on(async move {
            garb_graph_eth::start(
                pools.clone(),
                update_q_receiver,
                pending_update_q_receiver,
                Arc::new(RwLock::new(graph_routes)),
                Arc::new(std::sync::Mutex::new(single_routes_sender)),
                used_pools_shot_tx,
                graph_conifg,
            )
            .await
            .unwrap();
        });
    }));
    joins.push(std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();

        rt.block_on(async move {
            transactor(
                &mut routes_receiver,
                &mut single_routes_receiver,
                nodes.clone(),
            )
            .await
            .unwrap();
        });
    }));

    for join in joins {
        join.join().unwrap()
    }
    Ok(())
}

pub fn calculate_next_block_base_fee(block: Block<TxHash>) -> anyhow::Result<U256> {
    // Get the block base fee per gas
    let base_fee = block.base_fee_per_gas.unwrap();

    // Get the mount of gas used in the block
    let gas_used = block.gas_used;

    // Get the target gas used
    let mut target_gas_used = block.gas_limit / 2;
    target_gas_used = if target_gas_used == U256::zero() {
        U256::one()
    } else {
        target_gas_used
    };

    // Calculate the new base fee
    let new_base_fee = {
        if gas_used > target_gas_used {
            base_fee
                + ((base_fee * (gas_used - target_gas_used)) / target_gas_used) / U256::from(8u64)
        } else {
            base_fee
                - ((base_fee * (target_gas_used - gas_used)) / target_gas_used) / U256::from(8u64)
        }
    };

    // Add a random seed so it hashes differently
    let seed = rand::thread_rng().gen_range(0..9);
    Ok(new_base_fee + seed)
}

pub async fn transactor(
    rts: &mut kanal::AsyncReceiver<Backrun>,
    rt: &mut kanal::Receiver<Vec<ArbPath>>,
    nodes: NodeDispatcher,
) -> anyhow::Result<()> {
    let mut workers = vec![];
    let cores = num_cpus::get();
    let bundle_recievers = vec![
        "https://relay.flashbots.net".to_string(),
        "https://builder0x69.io/".to_string(),
        "https://rpc.beaverbuild.org/".to_string(),
        "https://rsync-builder.xyz/".to_string(),
        "https://relay.ultrasound.money/".to_string(),
        "https://agnostic-relay.net/".to_string(),
        "https://relayooor.wtf/".to_string(),
        "https://api.blocknative.com/v1/auction".to_string(),
        "https://api.edennetwork.io/v1/bundle".to_string(),
        "https://eth-builder.com".to_string(),
        "https://rpc.lightspeedbuilder.info/".to_string(),
        "https://api.securerpc.com/v1".to_string(),
        "https://BuildAI.net".to_string(),
        "https://rpc.payload.de".to_string(),
        "https://rpc.nfactorial.xyz/".to_string(),
    ];
    let mut bundle_handlers = vec![];
    let node_url = nodes.next_free();

    let provider = ethers_providers::Provider::<Ws>::connect(&node_url)
        .await
        .unwrap();
    #[cfg(feature = "ipc")]
    let provider = ethers_providers::Provider::<ethers_providers::Ipc>::connect_ipc("$HOME/.ethereum/geth.ipc").await.unwrap();

    let briber = Arc::new(abi::FlashbotsCheckAndSend::new(
        H160::from_str("0xc4595e3966e0ce6e3c46854647611940a09448d3").unwrap(),
        Arc::new(provider.clone()),
    ));

    for bundle_reciever in &bundle_recievers {
        let mut client = Arc::new(FlashbotsMiddleware::new(
            provider.clone(),
            Url::parse(bundle_reciever).unwrap(),
            BUNDLE_SIGNER_PRIVATE_KEY
                .clone()
                .parse::<LocalWallet>()
                .unwrap(),
        ));
        bundle_handlers.push(client)
    }
    for i in 0..cores {
        // backruns
        let node_url = nodes.next_free();

        let signer = PRIVATE_KEY.clone().parse::<LocalWallet>().unwrap();

        let provider = ethers_providers::Provider::<ethers_providers::Ws>::connect(&node_url).await.unwrap();

        #[cfg(feature = "ipc")]
        let provider = ethers_providers::Provider::<ethers_providers::Ipc>::connect_ipc("$HOME/.ethereum/geth.ipc").await.unwrap();

        // single transaction
        let routes = rt.clone();
        let client = Arc::new(
            SignerMiddleware::new_with_provider_chain(provider.clone(), signer.clone())
                .await
                .unwrap(),
        );
        let ap = client.clone();

        let bundle_handlers = bundle_handlers.clone();
        let nonce = Arc::new(tokio::sync::RwLock::new(U256::from(0)));

        let block: Arc<tokio::sync::RwLock<Block<H256>>> =
            Arc::new(tokio::sync::RwLock::new(Block::default()));

        let nonce_update = nonce.clone();
        let signer_wallet_address = signer.address();

        workers.push(tokio::runtime::Handle::current().spawn(async move {
            // keep updating nonce
            loop {
                if let Ok(n) = ap.get_transaction_count(signer_wallet_address, None).await {
                    let mut w = nonce_update.write().await;
                    *w = n;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }));
        let block_update = block.clone();
        let ap = client.clone();

        workers.push(tokio::runtime::Handle::current().spawn(async move {
            loop {
                if let Ok(Some(b)) = ap.get_block(BlockId::Number(BlockNumber::Latest)).await {
                    let mut w = block_update.write().await;
                    *w = b;
                } else {
                    error!("transactor > Error getting block number",);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }));

        let signer = Arc::new(signer);
        let signer_wallet_address = signer.address();

        workers.push(tokio::spawn(async move  {

                let bundle_signer = BUNDLE_SIGNER_PRIVATE_KEY.clone().parse::<LocalWallet>().unwrap();



                while let Ok(orders) = routes.recv() {

                    let mut futs = futures::stream::FuturesUnordered::new();
                    for opportunity in orders {
                        let nonce = nonce.clone();
                        let block = block.read().await.clone();
                        let signer = signer.clone();
                        let order = opportunity.tx.clone();
                        let gas_cost = opportunity.gas_cost;
                        for handler in &bundle_handlers {
                            let mut tx_request = order.clone();
                            let op = opportunity.clone();
                            let nonce_num = nonce.clone();
                            let block = block.clone();
                            let signer = signer.clone();
                            let mut handler = handler.clone();
                            futs.push(async move {
                                tx_request.to = Some(NameOrAddress::Address(CONTRACT_ADDRESS.clone()));
                                tx_request.from = Some(signer_wallet_address);
                                let n = * nonce_num.read().await ;
                                if block.base_fee_per_gas.is_none() {
                                    warn!("Skipping block not loaded {}. ->  {} {} ",i+1,gas_cost,opportunity.block_number);
                                    return
                                }
                                let max_fee = op.profit / gas_cost;
                                let balance = U256::from(50000000000000000 as u128);
                                let max_possible_fee = balance / gas_cost;
                                let base_fee = calculate_next_block_base_fee(block.clone()).unwrap();
                                tx_request.max_fee_per_gas = Some(max_fee.max(base_fee).min(max_possible_fee));
                                tx_request.gas = Some(gas_cost);
                                tx_request.nonce = Some(n.clone().checked_add(U256::from(0)).unwrap());
                                let blk = block.number.unwrap().as_u64();
                                drop(n);
                                drop(blk);

                                // profit doesn't cover tx_fees
                                if tx_request.max_fee_per_gas.unwrap() <= base_fee {
                                     warn!(
                                             "Skipping Unprofitable {}. ->  {} {} {:?} {:?}",
                                            i+1,
                                            tx_request.gas.unwrap(),
                                            opportunity.block_number,
                                            blk,
                                            tx_request.max_fee_per_gas.unwrap().checked_div(U256::from(10).pow(U256::from(9))).unwrap()
                                     );
                                    return
                                }
                                tx_request.max_priority_fee_per_gas = Some(tx_request.max_fee_per_gas.unwrap() - base_fee);

                                tx_request.value = None;


                                let typed_tx = TypedTransaction::Eip1559(tx_request.clone());
                                let tx_sig = signer.sign_transaction(&typed_tx).await.unwrap();
                                let signed_tx = typed_tx.rlp_signed(&tx_sig);
                                let mut bundle = vec![];
                                bundle.push(signed_tx);
                                warn!(
                                        "Trying {}. ->  {} {} {:?} {:?} {:?}",
                                        i+1,
                                        gas_cost,
                                        opportunity.block_number,
                                        blk,
                                        tx_request.max_priority_fee_per_gas.unwrap().checked_div(U256::from(10).pow(U256::from(9))).unwrap(),
                                        tx_request.max_fee_per_gas.unwrap().checked_div(U256::from(10).pow(U256::from(9))).unwrap()
                                );

                                let res = FlashBotsBundleHandler::simulate(bundle.clone(), &handler, opportunity.block_number, true).await;
                                        if let Some(res) = res {
                                            if res.transactions.iter().all(|tx| tx.error.is_none()) {
                                                for step in &op.result.steps {
                                                    info!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step.get_pool().await, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
                                                }
                                info!("\n\n\n");
                                                FlashBotsBundleHandler::submit(bundle, handler, opportunity.block_number, opportunity.block_number+3).await;

                                            }
                                        }
//                                FlashBotsBundleHandler::submit(bundle, handler, opportunity.block_number, opportunity.block_number+1).await;

                            });
                        }

                    }
                    futs.collect::<Vec<()>>().await;
                }

        }));
    }
    for worker in workers {
        worker.await.unwrap();
    }

    Ok(())
}

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}

#[derive(Serialize, Deserialize)]
pub struct JsonRpcRequest<'a, T> {
    id: u64,
    jsonrpc: &'a str,
    method: &'a str,
    params: T,
}

impl<'a, T> JsonRpcRequest<'a, T> {
    pub fn new(method: &'a str, params: T) -> Self {
        Self {
            id: 1,
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JsonRpcError {
    /// The error code
    pub code: i64,
    /// The error message
    pub message: String,
    /// Additional data
    pub data: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JsonRpcResponse<T> {
    pub(crate) id: u64,
    jsonrpc: String,
    #[serde(flatten)]
    pub data: JsonRpcResponseData<T>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum JsonRpcResponseData<R> {
    Error { error: JsonRpcError },
    Success { result: R },
}

impl<R> JsonRpcResponseData<R> {
    /// Consume response and return value
    pub fn into_result(self) -> Result<R, JsonRpcError> {
        match self {
            JsonRpcResponseData::Success { result } => Ok(result),
            JsonRpcResponseData::Error { error } => Err(error),
        }
    }
}

#[derive(Clone)]
pub struct FlashBotsBundleHandler {}

impl FlashBotsBundleHandler {
    async fn submit(
        txs: Vec<Bytes>,
        flashbots: Arc<FlashbotsMiddleware<Provider<Ws>, LocalWallet>>,
        from_block: u64,
        to_block: u64,
    ) {
        for block in from_block..to_block + 1 {
            let mut bundle = BundleRequest::new();
            for tx in &txs {
                bundle = bundle.push_transaction(tx.clone());
            }

            bundle = bundle
                .set_block(U64::from(block))
                .set_simulation_block(U64::from(block))
                .set_simulation_timestamp(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                );

            let res = flashbots.send_bundle(&bundle).await;
            if let Ok(res) = res {
                info!("{:?}", res.await);
                // let bundle_status = flashbots.get_bundle_stats(res.bundle_hash, res.block).await;
                // if let Ok(stats) = bundle_status {
                //         info!("{:?}",  stats);
                // }
            } else {
                debug!("Failed to submit transaction {:?}", res.err())
            }
        }
    }

    async fn simulate(
        txs: Vec<Bytes>,
        flashbots: &Arc<FlashbotsMiddleware<Provider<Ws>, LocalWallet>>,
        block: u64,
        only_successful: bool,
    ) -> Option<SimulatedBundle> {
        let mut bundle = BundleRequest::new();
        for tx in txs {
            bundle = bundle.push_transaction(tx);
        }

        bundle = bundle
            .set_block(U64::from(block))
            .set_simulation_block(U64::from(block))
            .set_simulation_timestamp(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );

        let simulation_result = flashbots.simulate_bundle(&bundle).await;

        if let Ok(res) = simulation_result {
            if only_successful {
                if res.transactions.iter().all(|tx| tx.error.is_none()) {
                    info!("{:?}", res);
                }
            } else {
                info!("{:?}", res);
            }
            return Some(res);
        } else {
            let err = simulation_result.unwrap_err();
            match &err {
                FlashbotsMiddlewareError::RelayError(e) => match e {
                    RelayError::JsonRpcError(err) => {
                        if err.message == "header not found".to_string() {
                            return None;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            // error!("Failed to simulate transaction {:?}", err)
        }
        None
    }
}

#[derive(Serialize, Deserialize, Clone)]
enum BundleHandlers {
    FlashBots(String, FlashBotsRequestParams),
    BlockVision(String, FlashBotsRequestParams),
    EthBuilder(String, FlashBotsRequestParams),
}

impl BundleHandlers {
    pub fn endpoint(&self) -> String {
        match self {
            Self::EthBuilder(url, _) | Self::FlashBots(url, _) | Self::BlockVision(url, _) => {
                url.clone()
            }
        }
    }

    pub fn set_txs(&mut self, txs: Vec<Bytes>) -> Self {
        match self {
            Self::EthBuilder(_, params)
            | Self::FlashBots(_, params)
            | Self::BlockVision(_, params) => params.txs = txs,
        }
        self.clone()
    }

    pub fn set_block(&mut self, block: u64) -> Self {
        match self {
            Self::EthBuilder(_, params)
            | Self::FlashBots(_, params)
            | Self::BlockVision(_, params) => params.block_number = U64::from(block),
        }
        self.clone()
    }
}
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FlashBotsRequestParams {
    txs: Vec<Bytes>,
    block_number: U64,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverting_tx_hashes: Option<Vec<Bytes>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replacement_uuid: Option<u128>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FlashBotsSimulateRequestParams {
    txs: Vec<Bytes>,
    block_number: U64,
    state_block_number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverting_tx_hashes: Option<Vec<Bytes>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replacement_uuid: Option<u128>,
}
