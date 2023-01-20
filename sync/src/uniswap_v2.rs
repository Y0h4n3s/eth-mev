//https://v2.info.uniswap.org/pairs

use serde::{Serialize, Deserialize};
use ethers::providers::{Provider, Http, Middleware};
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
	fn load_pools(&self) -> JoinHandle<()> {
		let metadata = self.metadata.clone();
		let pools = self.pools.clone();
		tokio::spawn(async move {
			let client = reqwest::Client::new();
			let response = client.post("https://api.thegraph.com/subgraphs/name/uniswap/uniswap-v2")
				.json(&json!({"operationName":"pairs","variables":{},"query":"query pairs {\n  pairs(first: 1000, orderBy: reserveUSD, orderDirection: desc) {\n    id\n    __typename\n    token0 {\n    id\n    symbol\n    name\n    decimals\n    totalLiquidity\n    derivedETH\n    __typename\n  }\n  token1 {\n    id\n    symbol\n    name\n    decimals\n    totalLiquidity\n    derivedETH\n    __typename\n  }\n  reserve0\n  reserve1\n  reserveUSD\n   volumeUSD\n  }\n}\n"}))
				.send()
				.await;
			if let Ok(resources) = response {
				let resp_values = resources.json::<ApiResponse<PairsResponse>>().await.unwrap();
				// let resp_values = resources.text().await.unwrap();
				for pair in resp_values.data.pairs {
					
					let pool = Pool {
						address: pair.id,
						x_address: pair.token0.id,
						fee_bps: 30,
						y_address: pair.token1.id,
						curve: None,
						curve_type: Curve::Uncorrelated,
						x_amount: pair.reserve0.parse::<f64>().unwrap() as u64,
						y_amount: pair.reserve1.parse::<f64>().unwrap() as u64,
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
		
		})
	}
}