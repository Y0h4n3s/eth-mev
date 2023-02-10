use crate::abi::{uniswap_v3::SwapFilter,IERC20};
use crate::types::UniSwapV3Pool;
use crate::types::UniSwapV3Token;
use crate::Meta;
use crate::{Curve, LiquidityProvider, LiquidityProviders};
use crate::{EventEmitter, EventSource, Pool};
use async_std::sync::Arc;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use coingecko::response::coins::CoinsMarketItem;
use ethers::abi::{Address, Uint};
use ethers::abi::{ParamType, Token};
use ethers::providers::{Http, Middleware, Provider,StreamExt};
use ethers::types::BlockNumber;
use ethers::types::ValueOrArray;
use ethers::types::{H160, H256, I256, U256, U64};
use ethers_providers::Ws;
use kanal::AsyncSender;
use num_bigfloat::BigFloat;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::error::Error;
use std::ops::Add;
use std::str::FromStr;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, LocalSet};
use uniswap_v3_math::tick_math::{MAX_SQRT_RATIO, MAX_TICK, MIN_SQRT_RATIO, MIN_TICK};

// Todo: add word in here to update and remove middleware use in simulate_swap
#[derive(Serialize, Deserialize,Decode, Encode, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct UniswapV3Metadata {
    pub factory_address: String,
    pub address: String,
    pub token_a: String,
    pub token_b: String,
    pub token_a_decimals: u8,
    pub token_b_decimals: u8,
    pub fee: u32,
    pub liquidity: u128,
    pub sqrt_price: String,
    pub tick: i32,
    pub tick_spacing: i32,
    pub liquidity_net: i128,
}

impl Meta for UniswapV3Metadata {}

#[derive(Default)]
pub struct StepComputations {
    pub sqrt_price_start_x_96: U256,
    pub tick_next: i32,
    pub initialized: bool,
    pub sqrt_price_next_x96: U256,
    pub amount_in: U256,
    pub amount_out: U256,
    pub fee_amount: U256,
}

pub struct CurrentState {
    amount_specified_remaining: I256,
    amount_calculated: I256,
    sqrt_price_x_96: U256,
    tick: i32,
    liquidity: u128,
    word_pos: i16,
}
impl UniswapV3Metadata {
    pub fn calculate_virtual_reserves(&self) -> (u128, u128) {
        let sqrt_price = U256::from_str(&self.sqrt_price).unwrap();
        let price = BigFloat::from_u128(
            (U256::from(sqrt_price.overflowing_mul(sqrt_price).0) >> 128).as_u128(),
        )
        .div(&BigFloat::from(2f64.powf(64.0)))
        .mul(&BigFloat::from_f64(10f64.powf(
            (self.token_a_decimals as i8 - self.token_b_decimals as i8) as f64,
        )));

        let sqrt_price = price.sqrt();
        let liquidity = BigFloat::from_u128(self.liquidity);

        //Sqrt price is stored as a Q64.96 so we need to left shift the liquidity by 96 to be represented as Q64.96
        //We cant right shift sqrt_price because it could move the value to 0, making divison by 0 to get reserve_x
        let liquidity = liquidity;

        let (reserve_0, reserve_1) = if !sqrt_price.is_zero() {
            let reserve_x = liquidity.div(&sqrt_price);
            let reserve_y = liquidity.mul(&sqrt_price);

            (reserve_x, reserve_y)
        } else {
            (BigFloat::from(0), BigFloat::from(0))
        };

        (
            reserve_0
                .to_u128()
                .expect("Could not convert reserve_0 to uin128"),
            reserve_1
                .to_u128()
                .expect("Could not convert reserve_1 to uin128"),
        )
    }

    // Calculate a human readable price from sqrt_ratio_x96.
    //
    // @dev sqrt_ratio_x96 = _token_a_price.pow(-2) * 2.pow(96)
    // @dev _token_a_price = (token_b_amount * 10.pow(token_b_decimals)) / (1 * 10.pow(token_a_decimals))
    //
    // @param { H160 } base_token
    // @returns { f64 } token_b_amount (swap through 1 token_a)
    //
    pub fn calculate_price(&self, base_token: String) -> f64 {
        let sqrt_price = U256::from_str(&self.sqrt_price).unwrap();
    
        let price = if self.token_b_decimals > self.token_a_decimals {
            BigFloat::from_str(
                &(U256::from(sqrt_price.overflowing_mul(sqrt_price).0) >> 128).to_string(),
            ).unwrap()
            .div(&BigFloat::from(2_u128.pow(64)))
            .div(&BigFloat::from(
                10_u64.pow((self.token_b_decimals as i8 - self.token_a_decimals as i8) as u32),
            ))
        } else {
            BigFloat::from_str(
                &(U256::from(sqrt_price.overflowing_mul(sqrt_price).0) >> 128).to_string(),
            ).unwrap()
            .div(&BigFloat::from(2_u128.pow(64)))
            .mul(&BigFloat::from(
                10_u64.pow((self.token_a_decimals as i8 - self.token_b_decimals as i8) as u32),
            ))
        };

        if self.token_a == base_token {
            price.to_f64()
        } else {
            1.0 / price.to_f64()
        }
    }
}

const UNISWAP_V3_DEPLOYMENT_BLOCK: u64 = 16369621;

pub const POOL_CREATED_EVENT_SIGNATURE: H256 = H256([
    120, 60, 202, 28, 4, 18, 221, 13, 105, 94, 120, 69, 104, 201, 109, 162, 233, 194, 47, 249, 137,
    53, 122, 46, 139, 29, 155, 43, 78, 107, 113, 24,
]);
pub struct UniSwapV3 {
    pub metadata: UniswapV3Metadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    subscribers: Arc<RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = Pool>>>>>>,
}

impl UniSwapV3 {
    pub fn new(metadata: UniswapV3Metadata) -> Self {
        Self {
            metadata,
            pools: Arc::new(RwLock::new(HashMap::new())),
            subscribers: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct PoolsResponse {
    pools: Vec<UniSwapV3Pool>,
}
#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse<T> {
    data: T,
}

// try to load from api and if it fails, load from local cache
#[async_trait]
impl LiquidityProvider for UniSwapV3 {
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
                Provider::<Ws>::connect("ws://89.58.31.215:8546")
                    .await
                    .unwrap(),
            );
            let current_block = eth_client.get_block_number().await.unwrap().0[0];

            let step = 100000_u64;
            let mut handles = vec![];

            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(cores));
            let mut pairs = Arc::new(RwLock::new(Vec::<UniSwapV3Pool>::new()));
            let mut indices: Arc<Mutex<VecDeque<(u64, u64)>>> =
                Arc::new(Mutex::new(VecDeque::new()));

            for i in (UNISWAP_V3_DEPLOYMENT_BLOCK..current_block).step_by(step as usize) {
                let mut w = indices.lock().await;
                w.push_back((i, i + step));
            }

            loop {
                let permit = permits.clone().acquire_owned().await.unwrap();
                let pairs = pairs.clone();
                let mut w = indices.lock().await;
                if w.len() == 0 {
                    break;
                }
                let span = w.pop_back();
                drop(w);

                if let Some((from_block, to_block)) = span {
                    let eth_client = eth_client.clone();
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
                                .map(|log| {
                                    let tokens = ethers::abi::decode(
                                        &[ParamType::Uint(32), ParamType::Address],
                                        &log.data,
                                    )
                                    .unwrap();
                                    match tokens.get(1).unwrap() {
                                        Token::Address(addr) => addr.clone(),
                                        _ => Default::default(),
                                    }
                                })
                                .collect::<Vec<H160>>()
                                .as_slice()
                                .chunks(76)
                            {
                                let pairs_data =
                                    crate::abi::uniswap_v3::get_pool_data_batch_request(
                                        chunk.to_vec(),
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
                    x_amount: 0,
                    y_amount: 0,
                    x_to_y: true,
                    provider: LiquidityProviders::UniswapV3(UniswapV3Metadata {
                        token_a: pair.token0.id.clone(),
                        token_b: pair.token1.id.clone(),
                        token_a_decimals: pair.token0_decimals,
                        token_b_decimals: pair.token1_decimals,
                        sqrt_price: pair.sqrt_price.clone(),
                        ..Default::default()
                    }),
                };
                let mut w = pools.write().await;
                w.insert(pool.address.clone(), pool);
            }
            println!(
                "{:?} Pools: {}",
                LiquidityProviders::UniswapV3(Default::default()),
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviders {
        LiquidityProviders::UniswapV3(Default::default())
    }
}

impl EventEmitter for UniSwapV3 {
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
                              ethers::contract::Contract::event_of_type::<SwapFilter>(&client)
                                    .from_block(latest_block)
                                    .address(ValueOrArray::Array(vec![pool.address.parse().unwrap()]));
    
                        let mut stream = event.subscribe_with_meta().await.unwrap().take(2);
    
                        while let Some(Ok((log, meta))) = stream.next().await {
                            if let Some(mut pool_meta) = match pool.clone().provider {
                                LiquidityProviders::UniswapV3(pool_meta) => Some(pool_meta),
                                _ => None
                            } {
                                pool_meta.sqrt_price = log.sqrt_price_x96.to_string();
                                pool.provider = LiquidityProviders::UniswapV3(pool_meta)
                            }
    
                            let mut subscribers = subscribers.write().await;
                            for subscriber in subscribers.iter_mut() {
                                let res = subscriber.send(Box::new(pool.clone())).await.map_err(|e| eprintln!("sync_service> UniswapV3 Send Error {:?}", e));
                            }
                        }
                    }));
                }
                futures::future::join_all(joins).await;
            });
        })
    }
}
