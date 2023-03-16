

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use crate::abi::{uniswap_v3::SwapFilter,IERC20};
use crate::types::UniSwapV3Pool;
use crate::types::UniSwapV3Token;
use crate::{LiquidityProviderId, Meta, PoolUpdateEvent, UniswapV3Calculator};
use crate::{Curve, LiquidityProvider, LiquidityProviders};
use crate::PendingPoolUpdateEvent;
use crate::{EventEmitter, EventSource, Pool};
use async_std::sync::Arc;
use async_trait::async_trait;
use ethers::abi::AbiDecode;
use bincode::{Decode, Encode};
use coingecko::response::coins::CoinsMarketItem;
use ethers::abi::{AbiEncode, Address, Uint};
use ethers::abi::{ParamType, Token};
use ethers::providers::{Http, Middleware, Provider,StreamExt};
use ethers::types::BlockNumber;
use ethers::types::ValueOrArray;
use ethers::types::{H160, H256, U256, U64};
use ethers_core::types::I256;
use ethers_providers::Ws;
use kanal::AsyncSender;
use num_bigfloat::BigFloat;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use ethers::core::utils::ParseUnits;
use std::collections::VecDeque;
use std::error::Error;
use std::ops::{Add, Div};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use ethers::core::k256::elliptic_curve::consts::{U2, U25};
use tokio::runtime::Runtime;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, LocalSet};
use uniswap_v3_math::sqrt_price_math::FIXED_POINT_96_RESOLUTION;
use uniswap_v3_math::tick_math::{MAX_SQRT_RATIO, MAX_TICK, MIN_SQRT_RATIO, MIN_TICK};
use tracing::{info, debug, trace, error};
use crate::abi::uniswap_v3::{get_complete_pool_data_batch_request, get_uniswap_v3_tick_data_batch_request, UniswapV3Pool, UniswapV3TickData};
use crate::node_dispatcher::NodeDispatcher;








const DEPLOYMENT_BLOCK: u64 = 12272147;

pub const POOL_CREATED_EVENT_SIGNATURE: H256 = H256(
    [131, 164, 143, 188, 252, 153, 19, 53, 49, 78, 116, 208, 73, 106, 171, 106, 25, 135, 233, 146, 221, 200, 93, 221, 188, 196, 214, 221, 110, 242, 233, 252]
);

const TVL_FILTER_LEVEL: i32 = 1;
// Todo: add word in here to update and remove middleware use in simulate_swap
#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct BalancerWeigtedMetadata {
    pub factory_address: String,
    pub address: String,
    pub tokens: Vec<String>,
    pub weights: Vec<u64>,
    pub token_a: String,
    pub token_b: String,
    pub token_a_amount: U256,
    pub token_b_amount: U256,
    pub token_a_decimals: u8,
    pub token_b_decimals: u8,
    pub fee: u32,
}

impl Meta for BalancerWeigtedMetadata {

}
pub struct BalancerWeighted {
    pub metadata: BalancerWeigtedMetadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>>>>,
    pending_subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>>>>,
    nodes: NodeDispatcher
}

impl BalancerWeighted {
    pub fn new(metadata: BalancerWeigtedMetadata, nodes: NodeDispatcher) -> Self {
        Self {
            metadata,
            pools: Arc::new(RwLock::new(HashMap::new())),
            subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            pending_subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            nodes
        }
    }
}




#[async_trait]
impl LiquidityProvider for BalancerWeighted {
    type Metadata = Box<dyn Meta>;
    fn get_metadata(&self) -> Self::Metadata {
        Box::new(self.metadata.clone())
    }
    async fn get_pools(&self) -> HashMap<String, Pool> {
        let lock = self.pools.read().await;
        lock.clone()
    }
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()> {
        let metadata = self.metadata.clone();
        let pools = self.pools.clone();
        let factory_address = H160::from_str(&self.metadata.factory_address).unwrap();
        let node_url = self.nodes.next_free();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let eth_client = Arc::new(
                Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap(),
            );
            let current_block = eth_client.get_block_number().await.unwrap().0[0];

            let step = 100000_u64;
            let mut handles = vec![];

            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(cores));
            let mut indices: Arc<Mutex<VecDeque<(u64, u64)>>> =
                Arc::new(Mutex::new(VecDeque::new()));

            for i in (DEPLOYMENT_BLOCK..current_block).step_by(step as usize) {
                let mut w = indices.lock().await;
                w.push_back((i, i + step));
            }

            loop {
                let permit = permits.clone().acquire_owned().await.unwrap();
                let mut w = indices.lock().await;
                if w.len() == 0 {
                    break;
                }
                let span = w.pop_back();
                drop(w);

                if let Some((from_block, to_block)) = span {
                    let eth_client = eth_client.clone();
                    let filter_tokens = filter_tokens.clone();
                    let pools = pools.clone();
                    handles.push(tokio::spawn(async move {
                        if let Ok(logs) = eth_client
                            .get_logs(
                                &ethers::types::Filter::new()
                                    .topic0(ValueOrArray::Value(POOL_CREATED_EVENT_SIGNATURE))
                                    .address(factory_address)
                                    .from_block(BlockNumber::Number(ethers::types::U64([
                                        from_block,
                                    ])))
                                    .to_block(BlockNumber::Number(ethers::types::U64([to_block]))),
                            )
                            .await
                        {
                            for chunk in logs
                                .iter()
                                .filter_map(|log| {
                                    let tokens = ethers::abi::decode(
                                        &[ParamType::Address],
                                        &log.data,
                                    )
                                        .unwrap();
                                    match tokens.get(0).unwrap() {
                                        Token::Address(addr) => Some(addr.clone()),
                                        _ => Default::default(),
                                    }
                                })
                                .collect::<Vec<H160>>()
                                .as_slice()
                                .chunks(9)
                            {
                                info!("{:?}", chunk);

                                let pairs_data =
                                    crate::abi::uniswap_v3::get_complete_pool_data_batch_request(
                                        chunk.to_vec(),
                                        eth_client.clone(),
                                    )
                                        .await;
                                if let Ok(mut pairs_data) = pairs_data {

                                    for meta in pairs_data {
                                        let min_0 = U256::from(10).pow(U256::from(meta.token_a_decimals as i32 + TVL_FILTER_LEVEL));
                                        let min_1 = U256::from(10).pow(U256::from(meta.token_b_decimals as i32 + TVL_FILTER_LEVEL));
                                        if meta.token_a_amount.lt(&min_0) || meta.token_b_amount.lt(&min_1)  {
                                            continue;
                                        }
                                        let pool = Pool {
                                            address: meta.address.clone(),
                                            x_address: meta.token_a.clone(),
                                            fee_bps: meta.fee as u64,
                                            y_address: meta.token_b.clone(),
                                            curve: None,
                                            curve_type: Curve::Uncorrelated,
                                            x_amount: meta.token_a_amount,
                                            y_amount: meta.token_b_amount,
                                            x_to_y: true,
                                            provider: LiquidityProviders::UniswapV3(meta),
                                        };

                                        let mut w = pools.write().await;
                                        w.insert(pool.address.clone(), pool);
                                    }
                                } else {
                                    info!("{:?}", pairs_data.unwrap_err())
                                }
                            }
                        } else {
                            error!("No pools")
                        }
                        drop(permit);
                    }));
                }
            }
            for handle in handles {
                handle.await;
            }


            info!(
                "{:?} Pools: {}",
                LiquidityProviderId::UniswapV3,
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviderId {
        LiquidityProviderId::UniswapV3
    }
}
fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}

impl EventEmitter<Box<dyn EventSource<Event=PoolUpdateEvent>>> for BalancerWeighted {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PoolUpdateEvent>>>>>> {
        self.subscribers.clone()
    }

    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.pools.clone();
        let subscribers = self.subscribers.clone();
        let node_url = self.nodes.next_free();

        std::thread::spawn(move || {
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();
            rt.block_on(async move {})
        })
    }
}
impl EventEmitter<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>> for BalancerWeighted {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>>>> {
        self.pending_subscribers.clone()
    }

    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.pools.clone();
        let subscribers = self.pending_subscribers.clone();
        let node_url = self.nodes.next_free();
        std::thread::spawn(move || {
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();
            rt.block_on(async move {})
        })
    }
}

