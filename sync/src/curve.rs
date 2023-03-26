

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
use crate::IPC_PATH;

#[serde(rename_all = "camelCase")]
#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct CurvePlainMetadata {
    pub id: String,
    pub factory_address: String,
    pub swap_fee: U256,
    pub address: String,
}

impl Meta for CurvePlainMetadata {

}
pub struct CurvePlain {
    pub metadata: CurvePlainMetadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    pub update_pools: Arc<Vec<[Arc<RwLock<Pool>>; 2]>>,
    subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>>>>,
    pending_subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>>>>,
    nodes: NodeDispatcher
}

impl CurvePlain {
    pub fn new(metadata: CurvePlainMetadata, nodes: NodeDispatcher) -> Self {
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

            let response = crate::abi::curve::get_pairs_batch_request(
                H160::from_str(&factory_address).unwrap(),
                U256::from(0),
                eth_client.clone(),
            )
                .await;

            if let Ok(pools) = response {
                info!("{:?}", pools);
                use crate::UniswapV2Metadata;
                let mut pairs = Arc::new(RwLock::new(Vec::<UniswapV2Metadata>::new()));

                for pair_chunk in pools.as_slice().chunks(67) {
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
            } else {
                info!("{:?}", response.unwrap_err())
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
                    let mut pl = pls[0].read().await.clone();

                    let subscribers = subscribers.clone();
                    let subscribers = subscribers.read().unwrap();
                    let sub = subscribers.first().unwrap().clone();
                    drop(subscribers);
                    let client = clnt.clone();
                    joins.push(tokio::runtime::Handle::current().spawn(async move {
                        let mut first = true;
                        loop {
                            tokio::time::sleep(Duration::from_millis(POLL_INTERVAL)).await;

                            let mut block = 0;
                            if let Some(mut data) = match pl.clone().provider {
                                LiquidityProviders::CurvePlain(pool_meta) => Some(pool_meta),
                                _ => None
                            } {

                            }
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
            return
        })
    }
}

