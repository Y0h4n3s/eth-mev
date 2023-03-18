

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use crate::abi::{ IERC20, Vault};
use crate::abi::VaultEvents::{SwapFilter, PoolBalanceChangedFilter, ExternalBalanceTransferFilter, PoolRegisteredFilter,PoolBalanceManagedFilter, FlashLoanFilter, };
use crate::types::UniSwapV3Pool;
use crate::types::UniSwapV3Token;
use crate::{abi, LiquidityProviderId, Meta, PoolUpdateEvent, UniswapV3Calculator};
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
use serde::{Deserialize, Serialize, Deserializer, Serializer};
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
use tracing::{info, debug, trace, error, warn};
use crate::abi::uniswap_v3::{get_complete_pool_data_batch_request, get_uniswap_v3_tick_data_batch_request, UniswapV3Pool, UniswapV3TickData};
use crate::node_dispatcher::NodeDispatcher;
use crate::POLL_INTERVAL;

fn from_float_str<'de, D>(deserializer: D) -> Result<U256, D::Error>
    where
        D: Deserializer<'de>,
{
    let s: &str = Deserialize::deserialize(deserializer)?;
    let float = s.parse::<f64>().unwrap_or(0.0) * 10_f64.powf(9.0);
    let uint = U256::from(float as u128);
    Ok(uint * U256::from(10).pow(U256::from(9)))

}


#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct FileBalancerWeightedPoolToken {
    pub address: String,
    #[serde(deserialize_with = "from_float_str")]
    pub weight: U256,
    pub decimals: u8,
    #[serde(deserialize_with = "from_float_str")]
    pub balance: U256,
}

#[serde(rename_all = "camelCase")]
#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct FileBalancerWeigtedMetadata {
    pub id: String,
    #[serde(deserialize_with = "from_float_str")]
    pub swap_fee: U256,
    pub address: String,
    pub tokens: Vec<FileBalancerWeightedPoolToken>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct BalancerWeightedPoolToken {
    pub address: String,
    pub weight: U256,
    pub decimals: u8,
    pub balance: U256,
}

impl From<FileBalancerWeightedPoolToken> for BalancerWeightedPoolToken {
    fn from(f: FileBalancerWeightedPoolToken) -> Self {
        Self {
            address: f.address,
            weight: f.weight,
            decimals: f.decimals,
            balance: f.balance
        }
    }
}
#[serde(rename_all = "camelCase")]
#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct BalancerWeigtedMetadata {
    pub id: String,
    pub factory_address: String,
    pub swap_fee: U256,
    pub address: String,
    pub tokens: Vec<BalancerWeightedPoolToken>,
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
    async fn set_pools(&self, pools: HashMap<String, Pool>)  {
        let mut lock = self.pools.write().await;
        *lock = pools;
    }
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()> {
        let metadata = self.metadata.clone();
        let pools = self.pools.clone();
        let factory_address = self.metadata.factory_address.clone();
        let node_url = self.nodes.next_free();
        tokio::spawn(async move {
            let file_pools = std::fs::read_to_string("balancer_weighted.json").unwrap();
            let eth_client = Arc::new(
                Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap(),
            );
            let pairs_data: Vec<FileBalancerWeigtedMetadata> = serde_json::from_str(&file_pools).unwrap();
            let meta_data = pairs_data.iter().cloned().map(|data| {
                BalancerWeigtedMetadata {
                    id: data.id,
                    factory_address: factory_address.clone(),
                    swap_fee: data.swap_fee,
                    address: data.address,
                    tokens: data.tokens.into_iter().map(|t| BalancerWeightedPoolToken::from(t)).collect::<Vec<BalancerWeightedPoolToken>>(),
                }
            }).collect::<Vec<BalancerWeigtedMetadata>>();


            for chunk in meta_data.chunks(65) {
                if let Ok(balances) = abi::balancer::get_complete_pool_data_batch_request(chunk.to_vec(), eth_client.clone()).await {
                    for status in balances {
                        // skip pools with > 2 tokens for now
                        if status.tokens.len() > 2 {
                            continue;
                        }
                        let data = pairs_data.iter().find(|d| d.id == status.id).unwrap().clone();
                        let mut tokens = vec![];
                        for i in 0..status.tokens.len() {
                            let mut existing = data.tokens.iter().find(|t| t.address == status.tokens[i]).unwrap().clone();
                            existing.balance = status.balances[i];
                            tokens.push(existing.clone());
                        }
                        let meta = BalancerWeigtedMetadata {
                            id: data.id,
                            factory_address: factory_address.clone(),
                            swap_fee: data.swap_fee,
                            address: data.address,
                            tokens: tokens.into_iter().map(|t| BalancerWeightedPoolToken::from(t)).collect::<Vec<BalancerWeightedPoolToken>>(),
                        };


                        let pool = Pool {
                            address: meta.address.clone(),
                            x_address: meta.tokens.first().unwrap().address.clone(),
                            fee_bps: 0,
                            y_address: meta.tokens.last().unwrap().address.clone(),
                            curve: None,
                            curve_type: Curve::Uncorrelated,
                            x_amount: meta.tokens.first().unwrap().balance.clone(),
                            y_amount: meta.tokens.last().unwrap().balance.clone(),
                            x_to_y: true,
                            provider: LiquidityProviders::BalancerWeighted(meta),
                        };
                        let mut w = pools.write().await;
                        w.insert(pool.address.clone(), pool);
                    }
                } else {
                    error!("Error loading BalancerWeighted Pool balances");
                }

            }



            info!(
                "{:?} Pools: {}",
                LiquidityProviderId::BalancerWeighted,
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviderId {
        LiquidityProviderId::BalancerWeighted
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
        let factory_address = self.metadata.factory_address.clone();

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
                let contract = Vault::new(Address::from_str("0xBA12222222228d8Ba445958a75a0704d566BF2C8").unwrap(), client.clone());

                let events = contract.events();
                let mut stream = events.stream().await.unwrap();

                while let Some(Ok(e)) = stream.next().await {
                    let client = client.clone();

                    let pools = pools.clone();
                    let factory_address = factory_address.clone();
                    let subscribers = subscribers.read().unwrap();
                    let sub = subscribers.first().unwrap().clone();
                    tokio::runtime::Handle::current().spawn(async move {
                        let pool_id = match e {
                            SwapFilter(data) => {
                                data.pool_id
                            }
                            PoolBalanceChangedFilter(data) => {
                                data.pool_id
                            }
                            PoolBalanceManagedFilter(data) => {
                                data.pool_id
                            }
                            _ => return
                        };
                        let hex_id = U256::from(pool_id).encode_hex();
                        let mut r = pools.read().await.clone();
                        if let Some((_, mut pool)) = r.iter_mut().find(|(_, p)| {
                            match &p.provider {
                                LiquidityProviders::BalancerWeighted(meta) => meta.id == hex_id,
                                _ => false
                            }
                        }) {
                            match pool.provider.clone() {
                                LiquidityProviders::BalancerWeighted(data) => {
                                    if let Ok(res) =  abi::balancer::get_complete_pool_data_batch_request(vec![data.clone()], client).await {
                                        let status = res.first().unwrap();
                                        let mut tokens = vec![];
                                        for i in 0..status.tokens.len() {
                                            let mut existing = data.tokens.iter().find(|t| t.address == status.tokens[i]).unwrap().clone();
                                            existing.balance = status.balances[i];
                                            tokens.push(existing.clone());
                                        }
                                        let meta = BalancerWeigtedMetadata {
                                            id: data.id,
                                            factory_address: factory_address.clone(),
                                            swap_fee: data.swap_fee,
                                            address: data.address,
                                            tokens: tokens,
                                        };
                                        pool.x_amount = meta.tokens.first().unwrap().balance;
                                        pool.y_amount = meta.tokens.last().unwrap().balance;

                                        pool.provider = LiquidityProviders::BalancerWeighted(meta);
                                        let mut w = pools.write().await;
                                        w.insert(pool.address.clone(), pool.clone());

                                        let event = PoolUpdateEvent {
                                            pool: pool.clone(),
                                            block_number: status.block_number,
                                            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                                        };

                                        let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> Balancer Weighted Send Error {:?}", e));

                                    }
                                }
                                _ => ()
                            }

                        }
                    });
                }
            })
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

