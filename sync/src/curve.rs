#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use crate::abi::{IERC20, Vault};
use crate::abi::VaultEvents::{SwapFilter, PoolBalanceChangedFilter, ExternalBalanceTransferFilter, PoolRegisteredFilter, PoolBalanceManagedFilter, FlashLoanFilter};
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
use ethers::providers::{Http, Middleware, Provider, StreamExt};
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
use crate::abi::curve::{get_complete_pool_data_batch_request};
use crate::node_dispatcher::NodeDispatcher;
use crate::POLL_INTERVAL;
use crate::IPC_PATH;

#[serde(rename_all = "camelCase")]
#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct CurvePlainMetadata {
    pub address: String,
    pub factory_address: String,
    pub tokens: Vec<String>,
    pub balances: Vec<U256>,
    pub decimals: Vec<u8>,
    pub fee: U256,
    pub amp: U256,
    pub block_number: u64,
}

impl Meta for CurvePlainMetadata {}

pub struct CurvePlain {
    pub metadata: CurvePlainMetadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    pub update_pools: Arc<Vec<[Arc<RwLock<Pool>>; 2]>>,
    subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PoolUpdateEvent>>>>>>,
    pending_subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>>>>,
    nodes: NodeDispatcher,
}

impl CurvePlain {
    pub fn new(metadata: CurvePlainMetadata, nodes: NodeDispatcher) -> Self {
        Self {
            metadata,
            pools: Arc::new(RwLock::new(HashMap::new())),
            update_pools: Arc::new(Vec::new()),
            subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            pending_subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            nodes,
        }
    }
}


#[async_trait]
impl LiquidityProvider for CurvePlain {
    type Metadata = Box<dyn Meta>;
    fn get_metadata(&self) -> Self::Metadata {
        Box::new(self.metadata.clone())
    }
    async fn get_pools(&self) -> HashMap<String, Pool> {
        let lock = self.pools.read().await;
        lock.clone()
    }
    async fn set_pools(&self, pools: HashMap<String, Pool>) {
        let mut lock = self.pools.write().await;
        *lock = pools;
    }
    fn set_update_pools(&mut self, pools: Vec<[Arc<RwLock<Pool>>; 2]>) {
        self.update_pools = Arc::new(pools);
    }
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()> {
        let metadata = self.metadata.clone();
        let pools = self.pools.clone();
        let factory_address = self.metadata.factory_address.clone();
        let node_url = self.nodes.next_free();
        tokio::spawn(async move {
            let eth_client = Arc::new(
                Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap(),
            );
            #[cfg(feature = "ipc")]
                let eth_client = Arc::new(ethers_providers::Provider::<ethers_providers::Ipc>::connect_ipc(&IPC_PATH.clone()).await.unwrap());


            let mut pls = vec![];

            let response = crate::abi::curve::get_pairs_batch_request(
                H160::from_str(&factory_address).unwrap(),
                U256::from(0),
                eth_client.clone(),
            )
                .await;
            if let Ok(p) = response {
                pls.extend(p)
            }


            let mut pairs = Arc::new(RwLock::new(Vec::<CurvePlainMetadata>::new()));

            for pair_chunk in pls.as_slice().chunks(20) {
                let pairs_data =
                    crate::abi::curve::get_complete_pool_data_batch_request(
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

            for pair in pairs.read().await.iter() {
                if pair.tokens.len() < 2 {
                    continue;
                }
                let pool = Pool {
                    address: pair.address.clone(),
                    x_address: pair.tokens.get(0).unwrap().clone(),
                    fee_bps: 0,
                    y_address: pair.tokens.get(1).unwrap().clone(),
                    curve: None,
                    curve_type: Curve::Stable,
                    x_amount: pair.balances.get(0).unwrap().clone(),
                    y_amount: pair.balances.get(1).unwrap().clone(),
                    x_to_y: true,
                    provider: LiquidityProviders::CurvePlain(pair.clone()),
                };

                let mut w = pools.write().await;
                w.insert(pool.address.clone(), pool);
            }


            info!(
                "{:?} Pools: {}",
                LiquidityProviderId::CurvePlain,
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviderId {
        LiquidityProviderId::CurvePlain
    }
}

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}

impl EventEmitter<Box<dyn EventSource<Event=PoolUpdateEvent>>> for CurvePlain {
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
                        let mut first = true;
                        loop {
                            let r = pool.read().await;
                            let pl = r.clone();
                            drop(r);
                            let (updated_meta, old_meta) = if let Some(mut pool_meta) = match pl.clone().provider {
                                LiquidityProviders::CurvePlain(pool_meta) => Some(pool_meta),
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
                                    error!("Failed to get {:?} updates", LiquidityProviderId::CurvePlain);
                                    continue
                                }
                            } else {
                                error!("Invalid Pool {:?} updates {}", LiquidityProviderId::CurvePlain, pl.clone());
                                continue
                            };
                            if old_meta.balances == updated_meta.balances  {
                                continue
                            }
                            for p in pls.iter() {
                                let mut w = p.write().await;
                                w.x_amount = updated_meta.balances.get(0).unwrap().clone();
                                w.y_amount = updated_meta.balances.get(1).unwrap().clone();
                                w.provider = LiquidityProviders::CurvePlain(updated_meta.clone());

                            }
                            let mut pl = pool.write().await;

                            pl.x_amount = updated_meta.balances.get(0).unwrap().clone();
                            pl.y_amount = updated_meta.balances.get(1).unwrap().clone();
                            pl.provider = LiquidityProviders::CurvePlain(updated_meta.clone());

                            let event = PoolUpdateEvent {
                                pool: pl.clone(),
                                block_number: updated_meta.block_number,
                                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            };
                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| info!("sync_service> CurvePlain Send Error {:?}", e));
                            tokio::time::sleep(Duration::from_millis(POLL_INTERVAL)).await;

                        }
                    }));
                }
                futures::future::join_all(joins).await;
            });
        })
    }
}

impl EventEmitter<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>> for CurvePlain {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>>>> {
        self.pending_subscribers.clone()
    }

    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.pools.clone();
        let subscribers = self.pending_subscribers.clone();
        let node_url = self.nodes.next_free();
        std::thread::spawn(move || {
            return;
        })
    }
}

