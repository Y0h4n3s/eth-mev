use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use std::sync::Arc;
use ethers::prelude::{Address, H160, H256, LocalWallet, SignerMiddleware};
use ethers::prelude::StreamExt;
use ethers::signers::Signer;
use ethers::types::{
    Block, BlockId, BlockNumber, NameOrAddress,
   TxHash, U256, U64,
};
use ethers::types::{Bytes};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers_flashbots::{
    BundleRequest, FlashbotsMiddleware, FlashbotsMiddlewareError, RelayError,
    SimulatedBundle,
};
use ethers_providers::{Middleware, Provider, Ws};
#[cfg(feature = "ipc")]
use ethers_providers::Ipc;
use once_cell::sync::Lazy;
use rand::Rng;
use tokio::runtime::Runtime;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tracing::{debug, error, info, warn};
use url::Url;

use garb_graph_eth::{GraphConfig};
use garb_graph_eth::single_arb::ArbPath;
use garb_sync_eth::{
    EventSource, LiquidityProviders, PendingPoolUpdateEvent, Pool,
    PoolUpdateEvent, SyncConfig,
};
use garb_sync_eth::node_dispatcher::NodeDispatcher;

mod abi;

static PROVIDERS: Lazy<Vec<LiquidityProviders>> = Lazy::new(|| {
    std::env::var("ETH_PROVIDERS")
        .unwrap_or_else(|_| {
            std::env::args()
                .nth(5)
                .unwrap_or("12,13,14,15,16,17,18,19".to_string())
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

#[cfg(feature = "ipc")]
static IPC_PATH: Lazy<String> = Lazy::new(|| {
    std::env::var("ETH_IPC_PATH").unwrap_or_else(|_| {
        std::env::args()
            .nth(6)
            .unwrap_or("/root/.ethereum/geth.ipc".to_string())
    })
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
        kanal::bounded_async::<Box<dyn EventSource<Event=PoolUpdateEvent>>>(10000);
    let (pending_update_q_sender, pending_update_q_receiver) =
        kanal::bounded_async::<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>(10000);
    // routes holds the routes that pass through an updated pool
    // this will be populated by the graph module when there is an updated pool
    let (single_routes_sender, mut single_routes_receiver) = kanal::bounded::<Vec<ArbPath>>(1000);
    let (used_pools_shot_tx, used_pools_shot_rx) =
        tokio::sync::oneshot::channel::<Vec<[Arc<RwLock<Pool>>; 2]>>();

    let from_file = std::env::args()
        .nth(1)
        .unwrap_or("true".to_string()).parse::<bool>().unwrap_or(true);
    let sync_config = SyncConfig {
        providers: PROVIDERS.clone(),
        from_file,
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

    let graph_config = GraphConfig {
        from_file,
        save_only: true,
    };
    joins.push(std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();

        rt.block_on(async move {
            garb_graph_eth::start(
                pools.clone(),
                update_q_receiver,
                pending_update_q_receiver,
                Arc::new(Mutex::new(single_routes_sender)),
                used_pools_shot_tx,
                graph_config,
            )
                .await
                .unwrap();
        });
    }));
    joins.push(std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();

        rt.block_on(async move {
            transactor(
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
    rt: &mut kanal::Receiver<Vec<ArbPath>>,
    nodes: NodeDispatcher,
) -> anyhow::Result<()> {
    let mut workers = vec![];
    let cores = num_cpus::get();
    let bundle_receivers = vec![
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
        let provider = ethers_providers::Provider::<Ipc>::connect_ipc(&IPC_PATH.clone()).await.unwrap();

    let briber = Arc::new(abi::FlashbotsCheckAndSend::new(
        H160::from_str("0xc4595e3966e0ce6e3c46854647611940a09448d3").unwrap(),
        Arc::new(provider.clone()),
    ));

    for bundle_receiver in &bundle_receivers {
        let client = Arc::new(FlashbotsMiddleware::new(
            provider.clone(),
            Url::parse(bundle_receiver).unwrap(),
            BUNDLE_SIGNER_PRIVATE_KEY
                .clone()
                .parse::<LocalWallet>()
                .unwrap(),
        ));
        bundle_handlers.push(client)
    }
    for i in 0..cores {
        let node_url = nodes.next_free();

        let signer = PRIVATE_KEY.clone().parse::<LocalWallet>().unwrap();

        let provider = ethers_providers::Provider::<Ws>::connect(&node_url).await.unwrap();
        #[cfg(feature = "ipc")]
            let provider = ethers_providers::Provider::<Ipc>::connect_ipc(&IPC_PATH.clone()).await.unwrap();

        // single transaction
        let routes = rt.clone();
        let client = Arc::new(
            SignerMiddleware::new_with_provider_chain(provider.clone(), signer.clone())
                .await
                .unwrap(),
        );
        let ap = client.clone();

        let bundle_handlers = bundle_handlers.clone();
        let nonce = Arc::new(RwLock::new(U256::from(0)));

        let block: Arc<RwLock<Block<H256>>> =
            Arc::new(RwLock::new(Block::default()));

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

        let briber = briber.clone();
        workers.push(tokio::spawn(async move {
            while let Ok(orders) = routes.recv() {
                let futs = futures::stream::FuturesUnordered::new();
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
                        let handler = handler.clone();
                        let briber = briber.clone();

                        futs.push(async move {
                            tx_request.to = Some(NameOrAddress::Address(CONTRACT_ADDRESS.clone()));
                            tx_request.from = Some(signer_wallet_address);
                            let n = *nonce_num.read().await;
                            if block.base_fee_per_gas.is_none() {
                                debug!("Skipping block not loaded {}. ->  {} {} ",i+1,gas_cost,opportunity.block_number);
                                return;
                            }
                            let gas_cost = gas_cost + U256::from(40000);
                            let bribe = (op.profit * U256::from(10)) / U256::from(100);
                            let profit = op.profit - bribe;
                            let max_fee = profit / gas_cost;
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
                                debug!(
                                             "Skipping Unprofitable {}. ->  {} {} {:?} {:?}",
                                            i+1,
                                            tx_request.gas.unwrap(),
                                            opportunity.block_number,
                                            blk,
                                            tx_request.max_fee_per_gas.unwrap().checked_div(U256::from(10).pow(U256::from(9))).unwrap()
                                     );
                                return;
                            }

                            tx_request.max_priority_fee_per_gas = Some(tx_request.max_fee_per_gas.unwrap() - base_fee);

                            tx_request.value = None;


                            let typed_tx = TypedTransaction::Eip1559(tx_request.clone());
                            let tx_sig = signer.sign_transaction(&typed_tx).await.unwrap();
                            let signed_tx = typed_tx.rlp_signed(&tx_sig);
                            let mut bundle = vec![];
                            bundle.push(signed_tx);
                            let mut typed_bribe_tx = briber.check_32_bytes_and_send(
                                H160::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap(),
                                Bytes::from_str("0x313ce567").unwrap(),
                                H256::from_str("0x0000000000000000000000000000000000000000000000000000000000000012").unwrap().0)
                                .gas(U256::from(50000)).gas_price(base_fee).value(bribe).from(signer_wallet_address).tx;
                            typed_bribe_tx.set_chain_id(U64::from(1));
                            typed_bribe_tx.set_nonce(typed_tx.nonce().unwrap() + U256::from(1));
                            let tx_sig = signer.sign_transaction(&typed_bribe_tx).await.unwrap();
                            let signed_tx = typed_bribe_tx.rlp_signed(&tx_sig);
                            bundle.push(signed_tx);
                            debug!(
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
                                    // FlashBotsBundleHandler::submit(bundle, handler, opportunity.block_number, opportunity.block_number+3).await;
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

#[derive(Clone)]
pub struct FlashBotsBundleHandler {}

impl FlashBotsBundleHandler {
    #![allow(dead_code)]
    async fn submit(
        txs: Vec<Bytes>,
        #[cfg(not(feature = "ipc"))]
        flashbots: Arc<FlashbotsMiddleware<Provider<Ws>, LocalWallet>>,
        #[cfg(feature = "ipc")]
        flashbots: Arc<FlashbotsMiddleware<Provider<ethers_providers::Ipc>, LocalWallet>>,
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
        #[cfg(not(feature = "ipc"))]
        flashbots: &Arc<FlashbotsMiddleware<Provider<Ws>, LocalWallet>>,
        #[cfg(feature = "ipc")]
        flashbots: &Arc<FlashbotsMiddleware<Provider<ethers_providers::Ipc>, LocalWallet>>,
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

