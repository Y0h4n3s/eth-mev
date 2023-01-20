//https://info.uniswap.org/pools
//'https://api.thegraph.com/subgraphs/name/uniswap/uniswap-v3'   -H 'authority: api.thegraph.com'   -H 'accept: */*'   -H 'accept-language: en-US,en;q=0.9'   -H 'content-type: application/json'   -H 'origin: https://info.uniswap.org'   -H 'referer: https://info.uniswap.org/'   -H 'sec-ch-ua: "Chromium";v="103", ".Not/A)Brand";v="99"'   -H 'sec-ch-ua-mobile: ?0'   -H 'sec-ch-ua-platform: "Linux"'   -H 'sec-fetch-dest: empty'   -H 'sec-fetch-mode: cors'   -H 'sec-fetch-site: cross-site'   -H 'user-agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/103.0.5060.134 Safari/537.36'   --data-raw '{"operationName":"pools","variables":{},"query":"query pools {\n  pools(\n   orderBy: totalValueLockedUSD\n    orderDirection: desc\n    subgraphError: allow\n  ) {\n    id\n    feeTier\n    liquidity\n    sqrtPrice\n    tick\n    token0 {\n      id\n      symbol\n      name\n      decimals\n      derivedETH\n      __typename\n    }\n    token1 {\n      id\n      symbol\n      name\n      decimals\n      derivedETH\n      __typename\n    }\n    token0Price\n    token1Price\n    volumeUSD\n    volumeToken0\n    volumeToken1\n    txCount\n    totalValueLockedToken0\n    totalValueLockedToken1\n    totalValueLockedUSD\n    __typename\n  }\n  bundles(where: {id: \"1\"}) {\n    ethPriceUSD\n    __typename\n  }\n}\n"}'   --compressed | jq > uniswapV3Pools.json


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
use crate::types::{UniSwapV3Pool};
use std::error::Error;
use reqwest;
use serde_json::json;
use crate::{Pool, EventSource, EventEmitter};

#[derive(Clone)]
pub struct UniswapV3Metadata {
	
}

impl Meta for UniswapV3Metadata {
	
}

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
	fn load_pools(&self) -> JoinHandle<()> {
		let metadata = self.metadata.clone();
		let pools = self.pools.clone();
		tokio::spawn(async move {
			let client = reqwest::Client::new();
			let response = client.post("https://api.thegraph.com/subgraphs/name/uniswap/uniswap-v3")
			                     .json(&json!({"operationName":"pools","variables":{},"query":"query pools {\n  pools(first: 1000, orderBy: totalValueLockedUSD,   orderDirection: desc, subgraphError: allow) {\n    id\n    feeTier\n    token0 {\n      id\n      symbol\n      name\n     decimals\n      derivedETH\n      __typename\n    }\n    token1 {\n      id\n      symbol\n      name\n      decimals\n      derivedETH\n      __typename\n    }\n    volumeUSD\n    totalValueLockedToken0\n    totalValueLockedToken1\n    totalValueLockedUSD\n    __typename\n  }\n}\n"}))
			                     .send()
			                     .await;
			if let Ok(resources) = response {
				let resp_values = resources.json::<ApiResponse<PoolsResponse>>().await.unwrap();
				// let resp_values = resources.text().await.unwrap();
				// println!("{:?}", resp_values);
				for pool in resp_values.data.pools {
					let pool = Pool {
						address: pool.id,
						x_address: pool.token0.id,
						fee_bps: 30,
						y_address: pool.token1.id,
						curve: None,
						curve_type: Curve::Uncorrelated,
						x_amount: pool.totalValueLockedToken0.parse::<f64>().unwrap() as u64,
						y_amount: pool.totalValueLockedToken1.parse::<f64>().unwrap() as u64,
						x_to_y: true,
						provider: LiquidityProviders::UniswapV3
					};
					let mut w = pools.write().await;
					w.insert(pool.address.clone(), pool);
				}
			} else {
				eprintln!("{:?}: {:?}", LiquidityProviders::UniswapV3, response.unwrap_err());
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
			// websocket listen to any event on pools
		})
	}
}