#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use hex::{FromHex, ToHex};
use crate::abi::{IERC20, UniswapV2Pair};
use crate::abi::{SyncFilter, UniswapV2Factory};
use bincode::{Decode, Encode};
use ethers::core::types::ValueOrArray;
use ethers::prelude::{abigen, Abigen, AddressOrBytes, Bytes, H160, H256, U256};
use ethers_providers::{Middleware, Provider, StreamExt, Ws};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, LocalSet};

use crate::types::UniSwapV2Pair;
use crate::{CpmmCalculator, LiquidityProviderId, Meta, PendingPoolUpdateEvent, POLL_INTERVAL, PoolUpdateEvent};
use crate::{Curve, LiquidityProvider, LiquidityProviders};
use crate::{EventEmitter, EventSource, Pool};
use async_std::sync::Arc;
use async_trait::async_trait;
use coingecko::response::coins::CoinsMarketItem;
use ethers::abi::{AbiEncode, AbiError, Address, Uint};
use kanal::AsyncSender;
use reqwest;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::io::Read;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use byte_slice_cast::AsByteSlice;
use ethers::types::serde_helpers;
use ethers::types::U64;
use tokio::runtime::Runtime;
use tracing::{debug, error, info, trace, warn};
use std::cmp::min;
use itertools::Itertools;
const UNISWAP_V2_ROUTER: &str = "0x7a250d5630b4cf539739df2c5dacb4c659f2488d";
pub(crate) const UNISWAP_UNIVERSAL_ROUTER: &str = "0xEf1c6E67703c7BD7107eed8303Fbe6EC2554BF6B";
const TVL_FILTER_LEVEL: i32 = 2;

#[derive(Serialize, Deserialize,Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct UniswapV2Metadata {
    pub address: String,
    pub factory_address: String,
    pub reserve0: U256,
    pub reserve1: U256,
    pub balance0: U256,
    pub balance1: U256,
    pub block_number: u64,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub token0: String,
    pub token1: String,
    pub fees: u32
}

impl Meta for UniswapV2Metadata {}

pub struct UniSwapV2 {
    pub metadata: UniswapV2Metadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    pub update_pools: Arc<Vec<[Arc<std::sync::RwLock<Pool>>; 2]>>,
    subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PoolUpdateEvent>>>>>>,
    pending_subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>>>>,
    nodes: NodeDispatcher,
    id: LiquidityProviderId
}

impl UniSwapV2 {
    pub fn new(metadata: UniswapV2Metadata, nodes: NodeDispatcher, id: LiquidityProviderId) -> Self {
        Self {
            metadata,
            pools: Arc::new(RwLock::new(HashMap::new())),
            update_pools: Arc::new(Vec::new()),
            subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            pending_subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            nodes,
            id
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
impl LiquidityProvider for UniSwapV2 {
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

    fn set_update_pools(&mut self, pools: Vec<[Arc<std::sync::RwLock<Pool>>; 2]>)  {
        self.update_pools = Arc::new(pools);
    }
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()> {
        let metadata = self.metadata.clone();
        let pools = self.pools.clone();
        let factory_address = H160::from_str(&self.metadata.factory_address).unwrap();
        let node_url = self.nodes.next_free();
        let id = self.id.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let eth_client = Arc::new(
                Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap(),
            );
            let factory = UniswapV2Factory::new(factory_address, eth_client.clone());

            let pairs_length: U256 = factory.all_pairs_length().call().await.unwrap();
            let step = 766;
            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(cores*2));
            let mut pairs = Arc::new(RwLock::new(Vec::<UniswapV2Metadata>::new()));
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
                    provider: LiquidityProviders::UniswapV2(pair.clone()),
                };
                if filter_tokens.contains(&pool.x_address) || filter_tokens.contains(&pool.y_address) {
                    continue;
                }
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
                id,
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviderId {
        self.id.clone()
    }
}


impl EventEmitter<Box<dyn EventSource<Event=PoolUpdateEvent>>> for UniSwapV2 {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PoolUpdateEvent>>>>>> {
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

                let mut provider = Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap();
                provider.set_interval(Duration::from_millis(POLL_INTERVAL));
                let clnt = Arc::new(
                        provider
                );
                let latest_block = clnt.get_block_number().await.unwrap();

                for p in pools.iter() {
                    let pls = p.clone();
                    let subscribers = subscribers.clone();
                    let mut pl = pls[0].read().unwrap().clone();
                    let subs = subscribers.read().unwrap();

                    let sub = subs.first().unwrap().clone();
                    drop(subs);
                    let client = clnt.clone();
                    joins.push(tokio::runtime::Handle::current().spawn(async move {


                        loop {
                            tokio::time::sleep(Duration::from_millis(POLL_INTERVAL)).await;

                            let (updated_meta, old_meta) = if let Some(mut pool_meta) = match pl.clone().provider {
                                LiquidityProviders::UniswapV2(pool_meta) => Some(pool_meta),
                                _ => None
                            } {
                                if let Ok(updates) =   get_complete_pool_data_batch_request(vec![H160::from_str(&pl.address).unwrap()], &client)
                                    .await {
                                    let mut updated_meta =
                                        updates
                                        .first()
                                        .unwrap()
                                        .to_owned();
                                    updated_meta.factory_address = pool_meta.factory_address.clone();
                                    (updated_meta, pool_meta)
                                } else {
                                    error!("Failed to get {:?} updates", LiquidityProviderId::UniswapV2);
                                    continue
                                }
                            } else {
                                continue
                            };
                            if old_meta.reserve0 == updated_meta.reserve0 && old_meta.reserve1 == updated_meta.reserve1 {
                                continue
                            }
                            for p in pls.iter() {
                                let mut w = p.write().unwrap();
                                w.x_amount = updated_meta.reserve0;
                                w.y_amount = updated_meta.reserve1;
                                w.provider = LiquidityProviders::UniswapV2(updated_meta.clone());

                            }
                            pl.x_amount = updated_meta.reserve0;
                            pl.y_amount = updated_meta.reserve1;
                            pl.provider = LiquidityProviders::UniswapV2(updated_meta.clone());
                            let event = PoolUpdateEvent {
                                pool: pl.clone(),
                                block_number: updated_meta.block_number,
                                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            };
                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| info!("sync_service> UniswapV2 Send Error {:?}", e));
                        }

                    }));
                }
                futures::future::join_all(joins).await;
            });
        })
    }
}

pub fn calculate_uniswap_v2_pair_address(a: &Address, b: &Address, factory: H160) -> anyhow::Result<Address> {
    let mut tokens = vec![a, b];
    tokens.sort();

    let mut data = [0u8; 40];
    data[0..20].copy_from_slice(tokens[0].as_bytes());
    data[20..].copy_from_slice(tokens[1].as_bytes());

    let salt = ethers::utils::keccak256(data);

    let init_code =
        <[u8; 32]>::from_hex("96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f")
            .map_err(|_| anyhow::Error::msg("Invalid init code hex"))?;

    Ok(ethers::utils::get_create2_address_from_hash(
        factory, salt, init_code,
    ))
}
fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}

pub fn calculate_out(in_: U256, swap_source_amount: U256, swap_destination_amount: U256) -> anyhow::Result<U256> {

    if swap_source_amount == U256::from(0) || swap_destination_amount == U256::from(0) {
        return Err(anyhow::Error::msg("Insufficient Liquidity"))
    }
    if in_ >= swap_source_amount {
        return Ok(swap_destination_amount);
    }
    let amount_in_with_fee = in_.saturating_mul(U256::from(97));
    let numerator = amount_in_with_fee
        .checked_mul(swap_destination_amount)
        .unwrap_or(U256::from(0));
    let denominator = ((swap_source_amount) * 100) + amount_in_with_fee;
    Ok((numerator / denominator))
}

pub fn calculate_in(out_: U256, swap_source_amount: U256, swap_destination_amount: U256) -> anyhow::Result<U256> {
    if swap_source_amount == U256::from(0) || swap_destination_amount == U256::from(0) || out_ >= swap_destination_amount {
        return Err(anyhow::Error::msg("Insufficient Liquidity"))
    }
    if out_ == swap_destination_amount {
        return Ok(swap_source_amount);
    }

    if let Some(numerator) = swap_source_amount.checked_mul( out_ * 100) {
        let denominator = (swap_destination_amount - out_) * U256::from((97) as u128);
        Ok((numerator / denominator) + 1)

    } else {
        Err(anyhow::Error::msg("Multiplication Overflow"))
    }

}
impl EventEmitter<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>> for UniSwapV2 {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>>>> {
        self.pending_subscribers.clone()
    }
    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.update_pools.clone();
        let subscribers = self.pending_subscribers.clone();
        let factory_address = H160::from_str(&self.metadata.factory_address).unwrap();
        let node_url = self.nodes.next_free();

        std::thread::spawn(move || {
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();

            rt.block_on(async move {
                let mut provider = Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap();
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
                        Err(e) => {
                            debug!("{:?}", e);
                        }
                        Ok(tx) => {
                            if tx.to != Some(H160::from_str(UNISWAP_V2_ROUTER).unwrap()) && tx.to != Some(H160::from_str(UNISWAP_UNIVERSAL_ROUTER).unwrap()) {
                                continue;
                            }
                            let client = client.clone();
                            let sub = sub.clone();
                            let pools = pools.clone();
                            tokio::task::spawn(async move {
                                let pools = pools
                                    .iter()
                                    .map(|p| p[0].read().unwrap().clone())
                                    .collect::<Vec<Pool>>();
                                if pools.len() <= 0 {
                                    return;
                                }
                                let factory_address = factory_address.clone();
                                let now = U256::from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
                                if tx.to == Some(H160::from_str(UNISWAP_V2_ROUTER).unwrap()) {


                                    // skip paths > 2 for now
                                    if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_exact_eth_for_tokens(&tx.input) {
                                        if decoded.path.len() > 2 {
                                            return;
                                        }
                                        if decoded.deadline < now {
                                            return;
                                        }
                                        //find associated pool
                                        let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());

                                        // path[0] is always weth

                                        if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                            let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                (pool.x_amount, pool.y_amount)
                                            } else {
                                                (pool.y_amount, pool.x_amount)
                                            };
                                            let amount_out = match calculate_out(tx.value, source_amount, dest_amount) {
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
                                                (mutated_pool.x_amount + tx.value, mutated_pool.y_amount.saturating_sub(amount_out))
                                            } else {
                                                (mutated_pool.x_amount.saturating_sub(amount_out), mutated_pool.y_amount + tx.value)
                                            };
                                            debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                            debug!("swapExactEthForTokens: {} {:?} {} {:?}", tx.value, decoded, pool, amount_out);
                                            let event = PendingPoolUpdateEvent {
                                                block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                                pool: mutated_pool,
                                                pending_tx: tx,
                                                timestamp: 0,
                                            };
                                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();
                                        } else {
                                            trace!("Pool {} not being tracked", pool_address);
                                        }
                                    } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_exact_tokens_for_tokens(&tx.input) {
                                        if decoded.path.len() > 2 {
                                            return;
                                        }

                                        if decoded.deadline < now {
                                            return;
                                        }
                                        let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());

                                        // path[0] is always weth

                                        if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                            let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                (pool.x_amount, pool.y_amount)
                                            } else {
                                                (pool.y_amount, pool.x_amount)
                                            };
                                            let amount_out = match calculate_out(decoded.amount_in, source_amount, dest_amount) {
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
                                                (mutated_pool.x_amount.saturating_sub(amount_out), mutated_pool.y_amount.saturating_add(decoded.amount_in))
                                            };
                                            debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                            debug!("swapExactTokensForTokens: {} {:?} {} {:?}", decoded.amount_in, decoded, pool, amount_out);
                                            let event = PendingPoolUpdateEvent {
                                                block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                                pool: mutated_pool,
                                                pending_tx: tx,
                                                timestamp: 0,
                                            };
                                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();
                                        } else {
                                            trace!("Pool {} not being tracked", pool_address);
                                        }
                                    } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_exact_tokens_for_eth(&tx.input) {
                                        if decoded.path.len() > 2 {
                                            return;
                                        }

                                        if decoded.deadline < now {
                                            return;
                                        }
                                        let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());
                                        if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                            let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                (pool.x_amount, pool.y_amount)
                                            } else {
                                                (pool.y_amount, pool.x_amount)
                                            };
                                            let amount_out = match calculate_out(decoded.amount_in, source_amount, dest_amount) {
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
                                                (mutated_pool.x_amount.saturating_sub(amount_out), mutated_pool.y_amount.saturating_add(decoded.amount_in))
                                            };
                                            debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                            debug!("swapExactTokensForEth: {} {:?} {} {:?}", tx.value, decoded, pool, amount_out);
                                            let event = PendingPoolUpdateEvent {
                                                block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                                pool: mutated_pool,
                                                pending_tx: tx,
                                                timestamp: 0,
                                            };
                                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();
                                        } else {
                                            trace!("Pool {} not being tracked", pool_address);
                                        }
                                    } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_tokens_for_exact_tokens(&tx.input) {
                                        if decoded.path.len() > 2 {
                                            return;
                                        }

                                        if decoded.deadline < now {
                                            return;
                                        }
                                        let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());
                                        if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                            let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                (pool.x_amount, pool.y_amount)
                                            } else {
                                                (pool.y_amount, pool.x_amount)
                                            };
                                            let amount_in = match calculate_in(decoded.amount_out, source_amount, dest_amount) {
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

                                            debug!("swapTokensForExactTokens: {:?}", decoded)
                                        } else {
                                            trace!("Pool {} not being tracked", pool_address);
                                        }
                                    } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_tokens_for_exact_eth(&tx.input) {
                                        if decoded.path.len() > 2 {
                                            return;
                                        }

                                        if decoded.deadline < now {
                                            return;
                                        }
                                        let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());
                                        if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                            let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                (pool.x_amount, pool.y_amount)
                                            } else {
                                                (pool.y_amount, pool.x_amount)
                                            };
                                            let amount_in = match calculate_in(decoded.amount_out, source_amount, dest_amount) {
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

                                            debug!("swapTokensForExactEth: {:?}", decoded)
                                        } else {
                                            trace!("Pool {} not being tracked", pool_address);
                                        }
                                    } else if let Ok(decoded) = crate::abi::decode_uniswap_router_swap_eth_for_exact_tokens(&tx.input) {
                                        if decoded.path.len() > 2 {
                                            return;
                                        }

                                        if decoded.deadline < now {
                                            return;
                                        }
                                        let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());
                                        if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                            let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                (pool.x_amount, pool.y_amount)
                                            } else {
                                                (pool.y_amount, pool.x_amount)
                                            };
                                            let amount_in = match calculate_in(decoded.amount_out, source_amount, dest_amount) {
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
                                            debug!("swapEthForExactTokens: {} {:?} {} {:?}", tx.value, decoded, pool, amount_in);
                                            let event = PendingPoolUpdateEvent {
                                                block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                                pool: mutated_pool,
                                                pending_tx: tx,
                                                timestamp: 0,
                                            };
                                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();
                                        } else {
                                            trace!("Pool {} not being tracked", pool_address);
                                        }
                                    } else {
                                        trace!("Useless transaction not decoded")
                                    }
                                } else if tx.to == Some(H160::from_str(UNISWAP_UNIVERSAL_ROUTER).unwrap()) {
                                    if let Ok(decoded) = crate::abi::decode_universal_router_execute(&tx.input) {
                                        if decoded.deadline < now {
                                            return;
                                        }
                                        for i in 0..decoded.commands.len() {
                                            let command = decoded.commands[i] & 0x3f;
                                            if command == 0x08 {
                                                let decoded: V2ExactInInput = V2ExactInInput::decode(&decoded.inputs[i]).unwrap();

                                                if decoded.path.len() > 2 {
                                                    return;
                                                }


                                                let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());

                                                // path[0] is always weth

                                                if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                        (pool.x_amount, pool.y_amount)
                                                    } else {
                                                        (pool.y_amount, pool.x_amount)
                                                    };
                                                    let amount_out = match calculate_out(decoded.amount_in, source_amount, dest_amount) {
                                                        Ok(amount) => amount,
                                                        Err(e) => {
                                                            debug!("{:?}", e);
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
                                                        (mutated_pool.x_amount.saturating_sub(amount_out), mutated_pool.y_amount.saturating_add(decoded.amount_in))
                                                    };
                                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                                    debug!("V2ExactInput: {} {:?} {} {:?}", decoded.amount_in, decoded, pool, amount_out);
                                                    let event = PendingPoolUpdateEvent {
                                                        block_number: tx.block_number.unwrap_or(U64::from(0)).as_u64(),
                                                        pool: mutated_pool,
                                                        pending_tx: tx.clone(),
                                                        timestamp: 0,
                                                    };
                                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();
                                                } else {
                                                    trace!("Pool {} not being tracked", pool_address);
                                                }
                                            } else if command == 0x09 {
                                                let decoded: V2ExactOutInput = V2ExactOutInput::decode(&decoded.inputs[i]).unwrap();

                                                if decoded.path.len() > 2 {
                                                    return;
                                                }

                                                let pool_address = hex_to_address_string(calculate_uniswap_v2_pair_address(&decoded.path[0], &decoded.path[1], factory_address).unwrap().encode_hex());
                                                if let Some(pool) = pools.iter().find(|p| p.address == pool_address) {
                                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                        (pool.x_amount, pool.y_amount)
                                                    } else {
                                                        (pool.y_amount, pool.x_amount)
                                                    };
                                                    let amount_in = match calculate_in(decoded.amount_out, source_amount, dest_amount) {
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
                                                        pending_tx: tx.clone(),
                                                        timestamp: 0,
                                                    };
                                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV2 Send Error {:?}", e)).unwrap();

                                                    debug!("V2ExactOut: {:?}", decoded)
                                                } else {
                                                    trace!("Pool {} not being tracked", pool_address);
                                                }
                                            }
                                        }
                                    } else {
                                        trace!("Couldn't decode universal router input {:?} {}", crate::abi::decode_universal_router_execute(&tx.input), &tx.input);
                                    }
                                } else {
                                    trace!("Transaction is not to uniswap v2, skipping...");
                                    return;
                                }
                            });
                        }
                    }
                }

            });
        })
    }
}

use ethers::abi::AbiDecode;
use crate::abi::uniswap_v2::get_complete_pool_data_batch_request;
use crate::node_dispatcher::NodeDispatcher;

#[derive(Serialize, Deserialize, Debug)]
pub struct V2ExactInInput {
    recipient: Address,
    pub amount_in: U256,
    pub amount_out_min: U256,
    payer_is_user: bool,
    pub path: Vec<Address>
}

#[derive(Serialize, Deserialize, Debug)]
pub struct V2ExactOutInput {
    recipient: Address,
    pub amount_out: U256,
    pub amount_in_max: U256,
    payer_is_user: bool,
    pub path: Vec<Address>
}

impl AbiDecode for V2ExactInInput {
    fn decode(bytes: impl AsRef<[u8]>) -> Result<Self, AbiError> {
        let bytes = bytes.as_ref().to_vec();

        let byte_chunks: Vec<[u8; 32]> = bytes.chunks(32).to_owned().map(|t| {
           let mut new: [u8; 32] = [0; 32];
                for i in 0..32 {
                    new[i] = t[i];
                }
            new
        }).collect();
        Ok(Self {
            recipient: Address::from_slice(&byte_chunks[0][12..32]),
            amount_in: U256::from_big_endian(&byte_chunks[1]),
            amount_out_min:  U256::from_big_endian(&byte_chunks[2]),
            payer_is_user: (byte_chunks[4][31] == 1),
            path: vec![Address::from_slice(&byte_chunks[6][12..32]), Address::from_slice(&byte_chunks[7][12..32])]

        })
    }
}


impl AbiDecode for V2ExactOutInput {
    fn decode(bytes: impl AsRef<[u8]>) -> Result<Self, AbiError> {
        let bytes = bytes.as_ref().to_vec();

        let byte_chunks: Vec<[u8; 32]> = bytes.chunks(32).to_owned().map(|t| {
           let mut new: [u8; 32] = [0; 32];
                for i in 0..32 {
                    new[i] = t[i];
                }
            new
        }).collect();
        Ok(Self {
            recipient: Address::from_slice(&byte_chunks[0][12..32]),
            amount_out: U256::from_big_endian(&byte_chunks[1]),
            amount_in_max:  U256::from_big_endian(&byte_chunks[2]),
            payer_is_user: (byte_chunks[4][31] == 1),
            path: vec![Address::from_slice(&byte_chunks[6][12..32]), Address::from_slice(&byte_chunks[7][12..32])]

        })
    }
}

