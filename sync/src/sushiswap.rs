#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]
use crate::abi::IERC20;
use crate::abi::{SyncFilter, UniswapV2Factory};
use bincode::{Decode, Encode};
use ethers::core::types::ValueOrArray;
use ethers::prelude::{abigen, Abigen, H160, H256, U256, U64};
use ethers_providers::{Middleware, Provider, StreamExt, Ws};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, LocalSet};
use crate::POLL_INTERVAL;
use crate::types::UniSwapV2Pair;
use crate::{CpmmCalculator, LiquidityProviderId, Meta, PendingPoolUpdateEvent, PoolUpdateEvent};
use crate::{Curve, LiquidityProvider, LiquidityProviders};
use crate::{EventEmitter, EventSource, Pool};
use async_std::sync::Arc;
use async_trait::async_trait;
use coingecko::response::coins::CoinsMarketItem;
use ethers::abi::{AbiEncode, Address, Uint};
use kanal::AsyncSender;
use reqwest;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tracing::{debug, error, info, trace};
use std::cmp::min;
use crate::node_dispatcher::NodeDispatcher;
use crate::uniswap_v2::UniswapV2Metadata;
use crate::IPC_PATH;
const SUSHISWAP_ROUTER: &str = "0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F";
const TVL_FILTER_LEVEL: i32 = -1;

#[derive(Serialize, Deserialize,Decode, Encode, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct SushiSwapMetadata {
    pub factory_address: String,
}

impl Meta for SushiSwapMetadata {}

pub struct SushiSwap {
    pub metadata: UniswapV2Metadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    pub update_pools: Arc<Vec<[Arc<RwLock<Pool>>; 2]>>,
    subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>>>>,
    pending_subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>>>>,
    nodes: NodeDispatcher
}

impl SushiSwap {
    pub fn new(metadata: UniswapV2Metadata, nodes: NodeDispatcher) -> Self {
        Self {
            metadata,
            pools: Arc::new(RwLock::new(HashMap::new())),
            update_pools: Arc::new(Vec::new()),
            subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            pending_subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            nodes
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct PairsResponse {
    pairs: Vec<UniSwapV2Pair>,
}
#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse<T> {
    data: T,
}

// try to load from api and if it fails, load from local cache
#[async_trait]
impl LiquidityProvider for SushiSwap {
    type Metadata = Box<dyn Meta>;
    fn get_metadata(&self) -> Self::Metadata {
        Box::new(self.metadata.clone())
    }
    async fn get_pools(&self) -> HashMap<String, Pool> {
        let lock = self.pools.read().await;
        lock.clone()
    }
    async fn set_pools(&self, pools: HashMap<String, Pool>)  {

        let mut lock = self.pools.write().await;
        *lock = pools;
    }
    fn set_update_pools(&mut self, pools: Vec<[Arc<RwLock<Pool>>; 2]>)  {
        self.update_pools = Arc::new(pools);
    }
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()> {
        let metadata = self.metadata.clone();
        let pools = self.pools.clone();
        let factory_address = H160::from_str(&self.metadata.factory_address).unwrap();
        let node_url = self.nodes.next_free();

        tokio::spawn(async move {
            let client = reqwest::Client::new();
            #[cfg(not(feature = "ipc"))]
            let eth_client = Arc::new(
                Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap(),
            );
            #[cfg(feature = "ipc")]
            let eth_client = Arc::new(ethers_providers::Provider::<ethers_providers::Ipc>::connect_ipc(&IPC_PATH.clone()).await.unwrap());
            let factory = UniswapV2Factory::new(factory_address, eth_client.clone());

            let pairs_length: U256 = factory.all_pairs_length().call().await.unwrap();
            let step = 766;
            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(cores*2));
            let mut pairs = Arc::new(RwLock::new(Vec::<crate::uniswap_v2::UniswapV2Metadata>::new()));
            let mut indices: Arc<Mutex<VecDeque<(usize, usize)>>> =
                Arc::new(Mutex::new(VecDeque::new()));

            for i in (0..pairs_length.as_usize()).step_by(step) {
                let mut w = indices.lock().await;
                w.push_back((i, min(i + step, pairs_length.as_usize() - 1)));
            }
            let mut handles = vec![];
            loop {
                let permit = permits.clone().acquire_owned().await.unwrap();
                let pairs = pairs.clone();
                let mut w = indices.lock().await;
                if w.len() == 0 {
                    break;
                }
                let span = w.pop_back();
                drop(w);

                if let Some((idx_from, idx_to)) = span {
                    let eth_client = eth_client.clone();
                    handles.push(tokio::spawn(async move {
                        let response = crate::abi::uniswap_v2::get_pairs_batch_request(
                            factory_address,
                            U256::from(idx_from),
                            U256::from(idx_to),
                            eth_client.clone(),
                        )
                            .await;

                        if let Ok(resources) = response {
                            for pair_chunk in resources.as_slice().chunks(67) {
                                let pairs_data =
                                    crate::abi::uniswap_v2::get_complete_pool_data_batch_request(
                                        pair_chunk.to_vec(),
                                        &eth_client,
                                    )
                                        .await;
                                if let Ok(mut pairs_data) = pairs_data {
                                    let mut w = pairs.write().await;
                                    w.append(&mut pairs_data);
                                } else {
                                    info!("{:?}", pairs_data.unwrap_err())
                                }
                            }
                        } else {
                            error!("{:?}", response.unwrap_err())
                        }
                        drop(permit);
                    }));
                }
            }
            for handle in handles {
                handle.await;
            }
            let len = pairs.read().await.len();
            for pair in pairs.read().await.iter() {
                let pool = Pool {
                    address: pair.address.clone(),
                    x_address: pair.token0.clone(),
                    fee_bps: 30,
                    y_address: pair.token1.clone(),
                    curve: None,
                    curve_type: Curve::Uncorrelated,
                    x_amount: pair.reserve0,
                    y_amount: pair.reserve1,
                    x_to_y: true,
                    provider: LiquidityProviders::SushiSwap(pair.clone()),
                };

                if filter_tokens.contains(&pool.x_address) || filter_tokens.contains(&pool.y_address) {
                    continue;
                }
                // atleast 0.1
                let min_0 = U256::from(10).pow(U256::from(pair.token0_decimals as i32 + TVL_FILTER_LEVEL));
                let min_1 = U256::from(10).pow(U256::from(pair.token1_decimals as i32 + TVL_FILTER_LEVEL));
                if pair.balance0.lt(&min_0) || pair.balance1.lt(&min_1) || pair.reserve1.lt(&min_1) || pair.reserve0.lt(&min_0) {
                    continue;
                }
                let mut w = pools.write().await;
                w.insert(pool.address.clone(), pool);
            }
            info!(
                "{:?} Pools: {}",
                LiquidityProviderId::SushiSwap,
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviderId {
        LiquidityProviderId::SushiSwap
    }
}



impl EventEmitter<Box<dyn EventSource<Event = PoolUpdateEvent>>> for SushiSwap {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>>>> {
        self.subscribers.clone()
    }
    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.update_pools.clone();
        let subscribers = self.subscribers.clone();
        let node_url = self.nodes.next_free();

        std::thread::spawn(move || {
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();
            rt.block_on(async move {
                let mut joins = vec![];
                #[cfg(not(feature = "ipc"))]
                let mut provider = Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap();
                #[cfg(feature = "ipc")]
                let mut provider = ethers_providers::Provider::<ethers_providers::Ipc>::connect_ipc(&IPC_PATH.clone()).await.unwrap();
                provider.set_interval(Duration::from_millis(POLL_INTERVAL));
                let clnt = Arc::new(
                        provider
                );
                let latest_block = clnt.get_block_number().await.unwrap();
                for p in pools.iter() {
                    let pls = p.clone();
                    let mut pl = Arc::new(RwLock::new(pls[0].read().await.clone()));

                    let subscribers = subscribers.clone();
                    let subscribers = subscribers.read().unwrap();
                    let subs = subscribers.first().unwrap().clone();
                    drop(subscribers);
                    let client = clnt.clone();
                    let pool = pl.clone();
                    let sub = subs.clone();
                    joins.push(tokio::runtime::Handle::current().spawn(async move {
                        let mut  first = true;
                        
                        loop {
                            let r = pool.read().await;
                            let pl = r.clone();
                            drop(r);
                            let (updated_meta, old_meta) = if let Some(mut pool_meta) = match pl.clone().provider {
                                LiquidityProviders::SushiSwap(pool_meta) => Some(pool_meta),
                                _ => None
                            } {
                                if let Ok(updates) = crate::abi::uniswap_v2::get_complete_pool_data_batch_request(vec![H160::from_str(&pl.address).unwrap()], &client)
                                    .await {
                                    let mut updated_meta = updates
                                        .first()
                                        .unwrap()
                                        .to_owned();
                                    updated_meta.factory_address = pool_meta.factory_address.clone();
                                    (updated_meta, pool_meta)
                                } else {
                                    error!("Failed to get {:?} updates", LiquidityProviderId::SushiSwap);
                                    continue
                                }

                            } else {
                                continue
                            };

                            if old_meta.reserve0 == updated_meta.reserve0 && old_meta.reserve1 == updated_meta.reserve1 {
                                continue
                            }
                             for p in pls.iter() {
                                let mut w = p.write().await;
                                w.x_amount = updated_meta.reserve0;
                                w.y_amount = updated_meta.reserve1;
                                w.provider = LiquidityProviders::SushiSwap(updated_meta.clone());

                            }
                            let mut pl = pool.write().await;

                            pl.x_amount = updated_meta.reserve0;
                            pl.y_amount = updated_meta.reserve1;
                            pl.provider = LiquidityProviders::SushiSwap(updated_meta.clone());
                            if first {
                                first = false;
                                continue
                            }
                            let event = PoolUpdateEvent {
                                pool: pl.clone(),
                                block_number: updated_meta.block_number,
                                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            };
                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| info!("sync_service> Sushiswap Send Error {:?}", e));
                            tokio::time::sleep(Duration::from_millis(5000)).await;

                        }
                    }));

                    let pool = pl.clone();
                    let client = clnt.clone();
                    let pls = p.clone();
                    let sub = subs.clone();

                    joins.push(tokio::runtime::Handle::current().spawn(async move {
                        let r = pool.read().await;
                        let pl = r.clone();
                        drop(r);
                        let contract = crate::abi::uniswap_v2_pair::UniswapV2Pair::new(H160::from_str(&pl.address).unwrap(), client.clone());
                        let events = contract.events();

                        let mut stream = events.stream().await.unwrap();
                        while let Some(_)  = stream.next().await {
                            let r = pool.read().await;
                            let pl = r.clone();
                            drop(r);
                            let (updated_meta, old_meta) = if let Some(mut pool_meta) = match pl.clone().provider {
                                LiquidityProviders::SushiSwap(pool_meta) => Some(pool_meta),
                                _ => None
                            } {
                                if let Ok(updates) = crate::abi::uniswap_v2::get_complete_pool_data_batch_request(vec![H160::from_str(&pl.address).unwrap()], &client)
                                    .await {
                                    let mut updated_meta = updates
                                        .first()
                                        .unwrap()
                                        .to_owned();
                                    updated_meta.factory_address = pool_meta.factory_address.clone();
                                    (updated_meta, pool_meta)
                                } else {
                                    error!("Failed to get {:?} updates", LiquidityProviderId::SushiSwap);
                                    continue
                                }

                            } else {
                                continue
                            };

                            if old_meta.reserve0 == updated_meta.reserve0 && old_meta.reserve1 == updated_meta.reserve1 {
                                continue
                            }
                            for p in pls.iter() {
                                let mut w = p.write().await;
                                w.x_amount = updated_meta.reserve0;
                                w.y_amount = updated_meta.reserve1;
                                w.provider = LiquidityProviders::SushiSwap(updated_meta.clone());

                            }
                            let mut pl = pool.write().await;

                            pl.x_amount = updated_meta.reserve0;
                            pl.y_amount = updated_meta.reserve1;
                            pl.provider = LiquidityProviders::SushiSwap(updated_meta.clone());
                            let event = PoolUpdateEvent {
                                pool: pl.clone(),
                                block_number: updated_meta.block_number,
                                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            };
                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| info!("sync_service> Sushiswap Send Error {:?}", e));
                        }
                    }));
                }
                futures::future::join_all(joins).await;
            });
        })
    }
}

impl EventEmitter<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>> for SushiSwap {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>>>> {
        self.pending_subscribers.clone()
    }
    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.update_pools.clone();
        let subscribers = self.pending_subscribers.clone();
        let factory_address = H160::from_str(&self.metadata.factory_address).unwrap();
        let node_url = self.nodes.next_free();

        std::thread::spawn(move || {
            return;
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();

            rt.block_on(async move {
                let mut provider = Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap();
                #[cfg(feature = "ipc")]
                let mut provider = ethers_providers::Provider::<ethers_providers::Ipc>::connect_ipc(&IPC_PATH.clone()).await.unwrap();
                provider.set_interval(Duration::from_millis(POLL_INTERVAL));
                let client = Arc::new(
                    provider
                );

                let latest_block = client.get_block_number().await.unwrap();
                let pending_stream = client.watch_pending_transactions().await.unwrap();
                let mut stream = pending_stream.transactions_unordered(usize::MAX);
                let subscribers = subscribers.read().unwrap();
                let sub = subscribers.first().unwrap().clone();
                drop(subscribers);
                while let Some(tx_result) = stream.next().await {
                    match tx_result {
                        Ok(tx) => {

                            if tx.to != Some(H160::from_str(SUSHISWAP_ROUTER).unwrap()) {
                                trace!("Transaction is not to sushiswap router, skipping...");
                                continue;
                            }
                            let client = client.clone();
                            let sub = sub.clone();
                            let pools = pools.clone();
                            tokio::task::spawn(async move {
                                let pools = futures::future::join_all(pools
                                    .iter()
                                    .map(|p| async {p[0].read().await.clone()})).await.into_iter()
                                    .collect::<Vec<Pool>>();
                            if pools.len() <= 0 {
                                return;
                            }
                            let factory_address = factory_address.clone();
                            let now = U256::from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
                            // skip paths > 2 for now
                            if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_exact_eth_for_tokens(&tx.input) {
                                if decoded.path.len() > 2 {
                                    return;
                                }
                                if decoded.deadline < now {
                                    return;
                                }
                                //find associated pool

                                // path[0] is always weth

                                if let Some(pool) = pools.iter().find(|p| (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) || (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex())) )  {
                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (pool.x_amount, pool.y_amount)
                                    } else {
                                        (pool.y_amount, pool.x_amount)
                                    };
                                    let amount_out = match crate::uniswap_v2::calculate_out(tx.value, source_amount, dest_amount) {
                                        Ok(amount) => amount,
                                        Err(e) => {
                                            trace!("{:?}", e);
                                            return
                                        }
                                    };

                                    if amount_out < decoded.amount_out_min {
                                        trace!("Output less than minimum");
                                        return
                                    }

                                    let mut mutated_pool = pool.clone();
                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (mutated_pool.x_amount.saturating_add(tx.value), mutated_pool.y_amount.saturating_sub(amount_out))
                                    } else {
                                        (mutated_pool.x_amount.saturating_sub(amount_out),mutated_pool.y_amount.saturating_add(tx.value))
                                    };
                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                    debug!("Sushiswap: swapExactEthForTokens: {} {:?} {} {:?}", tx.value, decoded, pool, amount_out);
                                    let event = PendingPoolUpdateEvent {
                                        pool: mutated_pool,
                                        block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                        pending_tx: tx,
                                        timestamp: 0,
                                    };
                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();


                                } else {
                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));
                                }
                            } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_exact_tokens_for_tokens(&tx.input) {
                                if decoded.path.len() > 2 {
                                    return;
                                }
                                if decoded.deadline < now {
                                    return;
                                }

                                // path[0] is always weth

                                if let Some(pool) = pools.iter().find(|p| (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) || (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex())) )  {
                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (pool.x_amount, pool.y_amount)
                                    } else {
                                        (pool.y_amount, pool.x_amount)
                                    };
                                    let amount_out = match crate::uniswap_v2::calculate_out(decoded.amount_in, source_amount, dest_amount) {
                                        Ok(amount) => amount,
                                        Err(e) => {
                                            trace!("{:?}", e);
                                            return
                                        }
                                    };

                                    if amount_out < decoded.amount_out_min {
                                        trace!("Output less than minimum");
                                        return
                                    }

                                    let mut mutated_pool = pool.clone();
                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (mutated_pool.x_amount.saturating_add(decoded.amount_in), mutated_pool.y_amount.saturating_sub(amount_out))
                                    } else {
                                        ( mutated_pool.x_amount.saturating_sub(amount_out), mutated_pool.y_amount.saturating_add(decoded.amount_in))
                                    };
                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                    debug!("Sushiswap: swapExactTokensForTokens: {} {:?} {} {:?}", decoded.amount_in, decoded, pool, amount_out);
                                    let event = PendingPoolUpdateEvent {
                                        block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                        pool: mutated_pool,
                                        pending_tx: tx,
                                        timestamp: 0,
                                    };
                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();


                                } else {
                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));
                                }
                            } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_exact_tokens_for_eth(&tx.input) {
                                if decoded.path.len() > 2 {
                                    return;
                                }

                                if decoded.deadline < now {
                                    return;
                                }

                                if let Some(pool) = pools.iter().find(|p| (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) || (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex())) )  {
                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (pool.x_amount, pool.y_amount)
                                    } else {
                                        (pool.y_amount, pool.x_amount)
                                    };
                                    let amount_out = match crate::uniswap_v2::calculate_out(decoded.amount_in, source_amount, dest_amount) {
                                        Ok(amount) => amount,
                                        Err(e) => {
                                            trace!("{:?}", e);
                                            return
                                        }
                                    };

                                    if amount_out < decoded.amount_out_min {
                                        trace!("Output less than minimum");
                                        return
                                    }
                                    let mut mutated_pool = pool.clone();
                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (mutated_pool.x_amount.saturating_add(decoded.amount_in), mutated_pool.y_amount.saturating_sub(amount_out))
                                    } else {
                                        (mutated_pool.x_amount.saturating_sub(amount_out), mutated_pool.y_amount.saturating_add(decoded.amount_in) )
                                    };
                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                    debug!("Sushiswap: swapExactTokensForEth: {} {:?} {} {:?}", tx.value, decoded, pool, amount_out);
                                    let event = PendingPoolUpdateEvent {
                                        block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                        pool: mutated_pool,
                                        pending_tx: tx,
                                        timestamp: 0,
                                    };
                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();


                                } else {
                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));
                                }
                            } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_tokens_for_exact_tokens(&tx.input) {
                                if decoded.path.len() > 2 {
                                    return;
                                }

                                if decoded.deadline < now {
                                    return;
                                }

                                if let Some(pool) = pools.iter().find(|p| (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) || (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex())) )  {
                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (pool.x_amount, pool.y_amount)
                                    } else {
                                        (pool.y_amount, pool.x_amount)
                                    };
                                    let amount_in = match crate::uniswap_v2::calculate_in(decoded.amount_out, source_amount, dest_amount) {
                                        Ok(amount) => amount,
                                        Err(e) => {
                                            trace!("{:?}", e);
                                            return
                                        }
                                    };
                                    if amount_in > decoded.amount_in_max {
                                        trace!("Insufficient amount for swap");
                                        return
                                    }
                                    let mut mutated_pool = pool.clone();
                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (mutated_pool.x_amount.saturating_add(amount_in), mutated_pool.y_amount.saturating_sub(decoded.amount_out))
                                    } else {
                                        ( mutated_pool.x_amount.saturating_sub(decoded.amount_out), mutated_pool.y_amount.saturating_add(amount_in))
                                    };
                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                    let event = PendingPoolUpdateEvent {
                                        block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                        pool: mutated_pool,
                                        pending_tx: tx,
                                        timestamp: 0,
                                    };
                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();

                                    debug!("Sushiswap: swapTokensForExactTokens: {:?}", decoded)

                                } else {
                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));
                                }
                            } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_tokens_for_exact_eth(&tx.input) {
                                if decoded.path.len() > 2 {
                                    return;
                                }

                                if decoded.deadline < now {
                                    return;
                                }

                                if let Some(pool) = pools.iter().find(|p| (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) || (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex())) )  {
                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (pool.x_amount, pool.y_amount)
                                    } else {
                                        (pool.y_amount, pool.x_amount)
                                    };
                                    let amount_in = match crate::uniswap_v2::calculate_in(decoded.amount_out, source_amount, dest_amount) {
                                        Ok(amount) => amount,
                                        Err(e) => {
                                            trace!("{:?}", e);
                                            return
                                        }
                                    };
                                    if amount_in > decoded.amount_in_max {
                                        trace!("Insufficient amount for swap");
                                        return
                                    }
                                    let mut mutated_pool = pool.clone();
                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (mutated_pool.x_amount.saturating_add(amount_in), mutated_pool.y_amount.saturating_sub(decoded.amount_out))
                                    } else {
                                        (mutated_pool.x_amount.saturating_sub(decoded.amount_out), mutated_pool.y_amount.saturating_add(amount_in))
                                    };
                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                    let event = PendingPoolUpdateEvent {
                                        block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                        pool: mutated_pool,
                                        pending_tx: tx,
                                        timestamp: 0,
                                    };
                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();

                                    debug!("Sushiswap: swapTokensForExactEth: {:?}", decoded)

                                } else {
                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));
                                }
                            } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_eth_for_exact_tokens(&tx.input) {
                                if decoded.path.len() > 2 {
                                    return;
                                }
                                if decoded.deadline < now {
                                    return;
                                }

                                if let Some(pool) = pools.iter().find(|p| (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) || (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex())) ) {
                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (pool.x_amount, pool.y_amount)
                                    } else {
                                        (pool.y_amount, pool.x_amount)
                                    };
                                    let amount_in = match crate::uniswap_v2::calculate_in(decoded.amount_out, source_amount, dest_amount) {
                                        Ok(amount) => amount,
                                        Err(e) => {
                                            trace!("{:?}", e);
                                            return
                                        }
                                    };
                                    if amount_in > tx.value {
                                        trace!("Insufficient amount for swap");
                                        return
                                    }
                                    let mut mutated_pool = pool.clone();
                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                        (mutated_pool.x_amount.saturating_add(amount_in), mutated_pool.y_amount.saturating_sub(decoded.amount_out))
                                    } else {
                                        (mutated_pool.x_amount.saturating_sub(decoded.amount_out), mutated_pool.y_amount.saturating_add(amount_in))
                                    };
                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                    debug!("Sushiswap: swapEthForExactTokens: {} {:?} {} {:?}", tx.value, decoded, pool, amount_in);
                                    let event = PendingPoolUpdateEvent {
                                        block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                        pool: mutated_pool,
                                        pending_tx: tx,
                                        timestamp: 0,
                                    };
                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();

                                } else {
                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));
                                }
                            } else {
                                debug!("Useless transaction not decoded")
                            }
                            });
                        }
                        Err(e) => {
                            debug!("{:?}", e);
                        }
                    }
                }

            });
        })
    }
}

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}