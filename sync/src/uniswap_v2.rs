//https://v2.info.uniswap.org/pairs

use serde::{Serialize, Deserialize};
use ethers_providers::{Provider, Ws, StreamExt, Middleware};
use ethers::prelude::{abigen, Abigen};
use ethers::core::types::ValueOrArray;
use crate::abi::SyncFilter;
use tokio::task::{JoinHandle, LocalSet};
use tokio::sync::{RwLock};
use tokio::runtime::Runtime;
use async_std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;
use kanal::AsyncSender;
use async_trait::async_trait;
use std::str::FromStr;
use crate::{LiquidityProvider, LiquidityProviders, Curve};
use crate::Meta;
use crate::types::{UniSwapV2Pair};
use std::error::Error;
use coingecko::response::coins::CoinsMarketItem;
use reqwest;
use serde_json::json;
use crate::{Pool, EventSource, EventEmitter};



#[derive(Clone)]
pub struct UniswapV2Metadata {

}

impl Meta for UniswapV2Metadata {

}

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
	data: T
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
	fn load_pools(&self, filter_markets: Vec<CoinsMarketItem>) -> JoinHandle<()> {
		let metadata = self.metadata.clone();
		let pools = self.pools.clone();
		tokio::spawn(async move {
			let client = reqwest::Client::new();
			let response = client.post("https://api.thegraph.com/subgraphs/name/uniswap/uniswap-v2")
				.json(&json!({"operationName":"pairs","variables":{},"query":"query pairs {\n  pairs(first: 50, orderBy: reserveUSD, orderDirection: desc) {\n    id\n    __typename\n    token0 {\n    id\n    symbol\n    name\n    decimals\n    totalLiquidity\n    derivedETH\n    __typename\n  }\n  token1 {\n    id\n    symbol\n    name\n    decimals\n    totalLiquidity\n    derivedETH\n    __typename\n  }\n  reserve0\n  reserve1\n  reserveUSD\n   volumeUSD\n  }\n}\n"}))
				.send()
				.await;
			if let Ok(resources) = response {
				let resp_values = resources.json::<ApiResponse<PairsResponse>>().await.unwrap();
				// let resp_values = resources.text().await.unwrap();
				for pair in resp_values.data.pairs {
					if !(
						filter_markets.iter().any(|market|
							  market.name == pair.token0.symbol
									|| market.name == pair.token0.name
									|| market.symbol == pair.token0.symbol
									|| market.symbol == pair.token0.name
									|| market.name == "W".to_string() + &pair.token0.symbol
									|| market.name == "W".to_string() + &pair.token0.name
									|| market.symbol == "W".to_string() + &pair.token0.symbol
									|| market.symbol == "W".to_string() + &pair.token0.name)
							  ||
							  filter_markets.iter().any(|market|
									market.name == pair.token1.symbol
										  || market.name == pair.token1.name
										  || market.symbol == pair.token1.symbol
										  || market.symbol == pair.token1.name
										  || market.name == "W".to_string() + &pair.token1.symbol
										  || market.name == "W".to_string() + &pair.token1.name
										  || market.symbol =="W".to_string() + &pair.token1.symbol
										  || market.symbol == "W".to_string() + &pair.token1.name) ) {
						// println!("sync service>Skipping pair {}-{} {}-{}", pair.token0.symbol, pair.token1.symbol, pair.token0.name, pair.token1.name);
						continue
					}
					let pool = Pool {
						address: pair.id,
						x_address: pair.token0.id,
						fee_bps: 30,
						y_address: pair.token1.id,
						curve: None,
						curve_type: Curve::Uncorrelated,
						x_amount: pair.reserve0.parse::<f64>().unwrap() as u128,
						y_amount: pair.reserve1.parse::<f64>().unwrap() as u128,
						x_to_y: true,
						provider: LiquidityProviders::UniswapV2
					};
					let mut w = pools.write().await;
					w.insert(pool.address.clone(), pool);
				}
				// println!("{:?}", resp_values);
				
			} else {
				eprintln!("{:?}: {:?}", LiquidityProviders::UniswapV2, response.unwrap_err());
			}
			println!("{:?} Pools: {}",LiquidityProviders::UniswapV2, pools.read().await.len());
		})
	}
	fn get_id(&self) -> LiquidityProviders {
		LiquidityProviders::UniswapV2
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
		std::thread::spawn( move || {
			let mut rt = Runtime::new().unwrap();
			let pools = pools.clone();
			let tasks = LocalSet::new();
			tasks.block_on(&mut rt, async move {
				let mut joins = vec![];
				let client = Arc::new(Provider::<Ws>::connect("ws://89.58.31.215:8546").await.unwrap());
				
				let latest_block = client.get_block_number().await.unwrap();
				
				for pool in pools.read().await.values() {
					let subscribers = subscribers.clone();
					let mut pool = pool.clone();
					let client = client.clone();
					joins.push(tokio::task::spawn_local(async move {
						let event = ethers::contract::Contract::event_of_type::<SyncFilter>(&client)
							  .from_block(latest_block)
							  .address(ValueOrArray::Array(vec![
								  pool.address.parse().unwrap()
							  ]));
						
						let mut stream = event.subscribe_with_meta().await.unwrap().take(2);
						
						while let Some(Ok((log, meta))) = stream.next().await {
							pool.x_amount = log.reserve_0;
							pool.y_amount = log.reserve_1;
							let mut subscribers = subscribers.write().await;
							for subscriber in subscribers.iter_mut() {
								subscriber.send(Box::new(pool.clone())).await.unwrap();
							}
							println!("{log:?}");
						}
					}));
				}
				futures::future::join_all(joins).await;
				
			});
		})
	}
}