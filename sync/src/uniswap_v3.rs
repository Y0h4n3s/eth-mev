
use coingecko::response::coins::CoinsMarketItem;
use serde::{Serialize, Deserialize};
use ethers::providers::{Provider, Http, Middleware};
use tokio::task::{JoinHandle, LocalSet};
use tokio::runtime::Runtime;
use async_std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;
use kanal::AsyncSender;
use async_trait::async_trait;
use std::str::FromStr;
use crate::{LiquidityProvider, LiquidityProviders, Curve};
use crate::Meta;
use crate::types::{UniSwapV3Pool};
use tokio::sync::{Mutex, RwLock, Semaphore};
use std::collections::VecDeque;
use std::error::Error;
use ethers::types::BlockNumber;
use ethers::types::ValueOrArray;
use reqwest;
use serde_json::json;
use ethers::abi::{Address, Uint};
use ethers::types::{H160, H256};
use ethers_providers::Ws;
use crate::{Pool, EventSource, EventEmitter};
use crate::abi::IERC20;
use ethers::abi::{ParamType, Token};
use crate::types::UniSwapV3Token;
#[derive(Clone)]
pub struct UniswapV3Metadata {
	pub factory_address: String
	
}

impl Meta for UniswapV3Metadata {
	
}

const UNISWAP_V3_DEPLOYMENT_BLOCK: u64 = 12369621;

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
	data: T
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
			let eth_client = Arc::new(Provider::<Ws>::connect("ws://89.58.31.215:8546").await.unwrap());
			let current_block = eth_client
				  .get_block_number()
				  .await
				  .unwrap()
				  .0[0];
			
			let step = 100000_u64;
			let mut handles = vec![];
			
			let cores = num_cpus::get();
			let permits = Arc::new(Semaphore::new(cores));
			let mut pairs = Arc::new(RwLock::new(Vec::<UniSwapV3Pool>::new()));
			let mut indices: Arc<Mutex<VecDeque<(u64, u64)>>> = Arc::new(Mutex::new(VecDeque::new()));
			
			for i in (UNISWAP_V3_DEPLOYMENT_BLOCK..current_block).step_by(step as usize) {
				let mut w = indices.lock().await;
				w.push_back((i, i+step));
			}
			
			loop {
				let permit = permits.clone().acquire_owned().await.unwrap();
				let pairs = pairs.clone();
				let mut w = indices.lock().await;
				if w.len() == 0 {
					break
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
										.from_block(BlockNumber::Number(ethers::types::U64([from_block])))
										.to_block(BlockNumber::Number(ethers::types::U64([to_block]))),
							  )
							  .await {
							for chunk in logs.iter().map(|log| {
								let tokens = ethers::abi::decode(&[ ParamType::Uint(32), ParamType::Address ], &log.data).unwrap();
								match tokens.get(1).unwrap() {
									Token::Address(addr) => {
										addr.clone()
									}
									_ => Default::default()
								}
							}).collect::<Vec<H160>>().as_slice().chunks(76) {
								let pairs_data = crate::abi::uniswap_v3::get_pool_data_batch_request(chunk.to_vec(), eth_client.clone()).await;
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
				if !(filter_tokens.iter().any(|token| token == &pair.token0.id) && filter_tokens.iter().any(|token| token == &pair.token1.id)) {
					continue
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
					provider: LiquidityProviders::UniswapV3
				};
				let mut w = pools.write().await;
				w.insert(pool.address.clone(), pool);
			}
			println!("{:?} Pools: {}",LiquidityProviders::UniswapV3, pools.read().await.len());
		})
	}
	fn get_id(&self) -> LiquidityProviders {
		LiquidityProviders::UniswapV3
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
		std::thread::spawn( move || {
			let mut rt = Runtime::new().unwrap();
			let pools = pools.clone();
			let tasks = LocalSet::new();
			tasks.block_on(&mut rt, async move {
				let mut joins = vec![];
				for pool in pools.read().await.values() {
					let subscribers = subscribers.clone();
					let mut pool = pool.clone();
					joins.push(tokio::task::spawn_local(async move {
					
					}));
				}
				futures::future::join_all(joins).await;
				
			});
		})
	}
}