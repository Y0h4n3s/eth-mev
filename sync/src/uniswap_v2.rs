use crate::abi::IERC20;
use crate::abi::{SyncFilter, UniswapV2Factory};
use bincode::{Decode, Encode};
use ethers::core::types::ValueOrArray;
use ethers::prelude::{abigen, Abigen, H160, H256, U256};
use ethers_providers::{Middleware, Provider, StreamExt, Ws};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, LocalSet};

use crate::types::UniSwapV2Pair;
use crate::{CpmmCalculator, LiquidityProviderId, Meta};
use crate::{Curve, LiquidityProvider, LiquidityProviders};
use crate::{EventEmitter, EventSource, Pool};
use async_std::sync::Arc;
use async_trait::async_trait;
use coingecko::response::coins::CoinsMarketItem;
use ethers::abi::{Address, Uint};
use kanal::AsyncSender;
use reqwest;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::str::FromStr;
use std::time::Duration;
use tokio::runtime::Runtime;

#[derive(Serialize, Deserialize,Decode, Encode, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct UniswapV2Metadata {
    pub factory_address: String,
}

impl Meta for UniswapV2Metadata {}

pub struct UniSwapV2 {
    pub metadata: UniswapV2Metadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    subscribers: Arc<RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = Pool>>>>>>,
}

impl UniSwapV2 {
    pub fn new(metadata: UniswapV2Metadata) -> Self {
        Self {
            metadata,
            pools: Arc::new(RwLock::new(HashMap::new())),
            subscribers: Arc::new(RwLock::new(Vec::new())),
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
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()> {
        let metadata = self.metadata.clone();
        let pools = self.pools.clone();
        let factory_address = H160::from_str(&self.metadata.factory_address).unwrap();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let eth_client = Arc::new(
                Provider::<Ws>::connect("ws://65.21.198.115:8546")
                    .await
                    .unwrap(),
            );
            let factory = UniswapV2Factory::new(factory_address, eth_client.clone());

            let pairs_length: U256 = factory.all_pairs_length().call().await.unwrap();
            let step = 766;
            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(cores));
            let mut pairs = Arc::new(RwLock::new(Vec::<UniSwapV2Pair>::new()));
            let mut indices: Arc<Mutex<VecDeque<(usize, usize)>>> =
                Arc::new(Mutex::new(VecDeque::new()));

            for i in (0..pairs_length.as_usize()).step_by(step) {
                let mut w = indices.lock().await;
                w.push_back((i, i + step));
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
                            for pair_chunk in resources.as_slice().chunks(127) {
                                let pairs_data =
                                    crate::abi::uniswap_v2::get_pool_data_batch_request(
                                        pair_chunk.to_vec(),
                                        eth_client.clone(),
                                    )
                                    .await;
                                if let Ok(mut pairs_data) = pairs_data {
                                    let mut w = pairs.write().await;
                                    w.append(&mut pairs_data);
                                } else {
                                    eprintln!("{:?}", pairs_data.unwrap_err())
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

            for pair in pairs.read().await.iter() {
                if !(filter_tokens.iter().any(|token| token == &pair.token0.id)
                    && filter_tokens.iter().any(|token| token == &pair.token1.id))
                {
                    continue;
                }
                let pool = Pool {
                    address: pair.id.clone(),
                    x_address: pair.token0.id.clone(),
                    fee_bps: 30,
                    y_address: pair.token1.id.clone(),
                    curve: None,
                    curve_type: Curve::Uncorrelated,
                    x_amount: pair.reserve0.parse::<u128>().unwrap(),
                    y_amount: pair.reserve1.parse::<u128>().unwrap(),
                    x_to_y: true,
                    provider: LiquidityProviders::UniswapV2(Default::default()),
                };
                let mut w = pools.write().await;
                w.insert(pool.address.clone(), pool);
            }
            println!(
                "{:?} Pools: {}",
                LiquidityProviderId::UniswapV2,
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviderId {
        LiquidityProviderId::UniswapV2
    }
}

impl EventEmitter for UniSwapV2 {
    type EventType = Box<dyn EventSource<Event = Pool>>;
    fn get_subscribers(&self) -> Arc<RwLock<Vec<AsyncSender<Self::EventType>>>> {
        self.subscribers.clone()
    }
    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.pools.clone();
        let subscribers = self.subscribers.clone();
        std::thread::spawn(move || {
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();
            let tasks = LocalSet::new();
            tasks.block_on(&mut rt, async move {
                let mut joins = vec![];
                let client = Arc::new(
                    Provider::<Ws>::connect("ws://89.58.31.215:8546")
                        .await
                        .unwrap(),
                );

                let latest_block = client.get_block_number().await.unwrap();

                for pool in pools.read().await.values() {
                    let subscribers = subscribers.clone();
                    let mut pool = pool.clone();
                    let client = client.clone();
                    joins.push(tokio::task::spawn_local(async move {
                        let event =
                            ethers::contract::Contract::event_of_type::<SyncFilter>(&client)
                                .from_block(latest_block)
                                .address(ValueOrArray::Array(vec![pool.address.parse().unwrap()]));

                        let mut stream = event.subscribe_with_meta().await.unwrap().take(2);

                        while let Some(Ok((log, meta))) = stream.next().await {
                            pool.x_amount = log.reserve_0;
                            pool.y_amount = log.reserve_1;
                            let mut subscribers = subscribers.write().await;
                            for subscriber in subscribers.iter_mut() {
                                let res = subscriber.send(Box::new(pool.clone())).await.map_err(|e| eprintln!("sync_service> UniswapV2 Send Error {:?}", e));
                            }
                        }
                    }));
                }
                futures::future::join_all(joins).await;
            });
        })
    }
}
