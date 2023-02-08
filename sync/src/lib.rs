mod abi;
mod events;
mod types;
mod uniswap_v2;
mod uniswap_v3;

use crate::events::EventEmitter;
use crate::uniswap_v2::UniswapV2Metadata;
use crate::uniswap_v3::UniswapV3Metadata;
use async_std::sync::Arc;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use coingecko::response::coins::CoinsMarketItem;
use ethers::types::{H160, H256, U256};
use ethers_providers::{Provider, Ws};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::*;
use std::str::FromStr;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
#[async_trait]
pub trait LiquidityProvider: EventEmitter {
    type Metadata;
    fn get_metadata(&self) -> Self::Metadata;
    async fn get_pools(&self) -> HashMap<String, Pool>;
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()>;
    fn get_id(&self) -> LiquidityProviders;
}

#[derive(Decode, Encode, Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LiquidityProviders {
    UniswapV2(UniswapV2Metadata),
    UniswapV3(UniswapV3Metadata),
}

impl<T: Into<String>> From<T> for LiquidityProviders {
    fn from(value: T) -> Self {
        match value.into().as_str() {
            "1" => LiquidityProviders::UniswapV2(Default::default()),
            "2" => LiquidityProviders::UniswapV3(Default::default()),
            val => match val.split("(").collect::<Vec<&str>>().get(0) {
                Some(v) => match *v {
                    "UniswapV2" => LiquidityProviders::UniswapV2(Default::default()),
                    "UniswapV3" => LiquidityProviders::UniswapV3(Default::default()),
                    _ => panic!("Invalid Liquidity Provider {}", v),
                },
                None => panic!("Invalid Liquidity Provider {}", val),
            },
        }
    }
}

impl From<LiquidityProviders> for u8 {
    fn from(value: LiquidityProviders) -> Self {
        match value {
            LiquidityProviders::UniswapV2(_) => 1,
            LiquidityProviders::UniswapV3(_) => 2,
        }
    }
}
pub trait EventSource: Send {
    type Event: Send;
    fn get_event(&self) -> Self::Event;
}
pub type BoxedLiquidityProvider = Box<
    dyn LiquidityProvider<Metadata = Box<dyn Meta>, EventType = Box<dyn EventSource<Event = Pool>>>
        + Send,
>;

impl LiquidityProviders {
    pub async fn build_calculator(&self) -> Box<dyn Calculator> {
        match self {
            LiquidityProviders::UniswapV2(meta) => Box::new(CpmmCalculator::new()),
            LiquidityProviders::UniswapV3(meta) => {
                Box::new(UniswapV3Calculator::new(meta.clone()).await)
            }
            _ => panic!("Invalid liquidity provider"),
        }
    }
    pub fn build(&self) -> BoxedLiquidityProvider {
        match self {
            LiquidityProviders::UniswapV2(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(metadata))
            }
            LiquidityProviders::UniswapV3(meta) => {
                let metadata = UniswapV3Metadata {
                    factory_address: "0x1F98431c8aD98523631AE4a59f267346ea31F984".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v3::UniSwapV3::new(metadata))
            }
            _ => panic!("Invalid liquidity provider"),
        }
    }
}

#[derive(Decode, Encode, Debug, Clone, Hash, Eq, PartialEq)]
pub enum Curve {
    Uncorrelated,
    Stable,
}

#[derive(Decode, Encode, Debug, Clone, Eq)]
pub struct Pool {
    pub address: String,
    pub x_address: String,
    pub y_address: String,
    pub curve: Option<String>,
    pub curve_type: Curve,
    pub fee_bps: u64,
    pub x_amount: u128,
    pub y_amount: u128,
    pub x_to_y: bool,
    pub provider: LiquidityProviders,
}

impl Hash for Pool {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.address.hash(state);
        self.x_address.hash(state);
        self.y_address.hash(state);
        self.provider.hash(state);
        self.curve_type.hash(state);
    }
}

impl PartialEq for Pool {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
            && self.x_address == other.x_address
            && self.y_address == other.y_address
            && self.provider == other.provider
            && self.curve_type == other.curve_type
    }
}
impl EventSource for Pool {
    type Event = Self;
    fn get_event(&self) -> Self::Event {
        self.clone()
    }
}

impl From<&Pool> for Pool {
    fn from(pool: &Pool) -> Self {
        Self {
            address: pool.address.clone(),
            x_address: pool.x_address.clone(),
            y_address: pool.y_address.clone(),
            curve: pool.curve.clone(),
            curve_type: pool.curve_type.clone(),
            x_amount: pool.x_amount,
            y_amount: pool.y_amount,
            x_to_y: pool.x_to_y,
            provider: pool.provider.clone(),
            fee_bps: pool.fee_bps,
        }
    }
}

impl Display for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pool {{ \n\tProvider: {:?}\n\taddress: {}\n\tx_address: {}\n\ty_address: {}\n\tis_x_to_y: {}\n\t", self.provider, self.address,self.x_address, self.y_address,  self.x_to_y)
    }
}

impl CpmmCalculator {
    fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Calculator for CpmmCalculator {
    async fn calculate_out(&self, in_: u128, pool: &Pool) -> anyhow::Result<u128> {
        let swap_source_amount = if pool.x_to_y {
            pool.x_amount
        } else {
            pool.y_amount
        };
        let swap_destination_amount = if pool.x_to_y {
            pool.y_amount
        } else {
            pool.x_amount
        };
        if in_ >= swap_source_amount {
            return Ok(swap_destination_amount);
        }
        let amount_in_with_fee = (in_ as u128) * ((10000 - pool.fee_bps) as u128);
        let numerator = amount_in_with_fee
            .checked_mul(swap_destination_amount as u128)
            .unwrap_or(0);
        let denominator = ((swap_source_amount as u128) * 10000) + amount_in_with_fee;
        Ok((numerator / denominator) as u128)
    }
}

#[async_trait]
pub trait Calculator {
    async fn calculate_out(&self, in_: u128, pool: &Pool) -> anyhow::Result<u128>;
}
pub struct CpmmCalculator {}

pub struct UniswapV3Calculator {
    meta: UniswapV3Metadata,
    middleware: Arc<Provider<Ws>>,
}

impl UniswapV3Calculator {
    pub async fn new(meta: UniswapV3Metadata) -> Self {
        Self {
            middleware: Arc::new(
                Provider::<Ws>::connect("ws://89.58.31.215:8546")
                    .await
                    .unwrap(),
            ),
            meta,
        }
    }
}
#[async_trait]
impl Calculator for UniswapV3Calculator {
    async fn calculate_out(&self, in_: u128, pool: &Pool) -> anyhow::Result<u128> {
        let token_in = if pool.x_to_y {
            H160::from_str(&pool.x_address)?
        } else {
            H160::from_str(&pool.y_address)?
        };
        Ok(self
            .meta
            .simulate_swap(token_in, U256::from(in_), self.middleware.clone())
            .await?
            .as_u128())
    }
}
pub trait Meta {}

pub struct SyncConfig {
    pub providers: Vec<LiquidityProviders>,
}

pub async fn start(
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    updated_q: kanal::AsyncSender<Box<dyn EventSource<Event = Pool>>>,
    config: SyncConfig,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    // collect top tokens by market cap
    let coingecko_client = coingecko::CoinGeckoClient::new("https://api.coingecko.com/api/v3");
    let mut markets_list = vec![];
    let all_coins = coingecko_client.coins_list(true).await?;
    let mut high_volume_tokens: Vec<String> = vec![];
    for i in 1..10 {
        let coin_list = coingecko_client
            .coins_markets::<String>(
                "usd",
                &[],
                Some("ethereum-ecosystem"),
                coingecko::params::MarketsOrder::VolumeDesc,
                250,
                i,
                false,
                &[],
            )
            .await?;
        markets_list.extend(coin_list);
    }
    for market in markets_list {
        if let Some(coin_info) = all_coins.iter().find(|coin| coin.id == market.id) {
            if let Some(platforms) = &coin_info.platforms {
                if platforms.contains_key(&"ethereum".to_string()) {
                    if let Some(contract) = platforms.get("ethereum").unwrap() {
                        high_volume_tokens.push(contract.clone())
                    }
                }
            }
        }
    }
    let mut join_handles = vec![];
    let mut amms = vec![];
    for provider in config.providers {
        let mut amm = provider.build();
        join_handles.push(amm.load_pools(high_volume_tokens.clone()));
        amm.subscribe(updated_q.clone()).await;
        amms.push(amm);
    }

    // Make sure pools are loaded before starting the listener
    println!("Loading Pools... {}", high_volume_tokens.len());
    for handle in join_handles {
        handle.await?;
    }
    let mut loaded_pools = HashMap::new();
    for amm in &amms {
        let pools = amm.get_pools().await;
        for (addr, pool) in pools {
            loaded_pools.insert(addr, pool);
        }
    }

    let mut pools = pools.write().await;
    *pools = loaded_pools;
    std::mem::drop(pools);

    let mut emitters = vec![];
    for amm in &amms {
        let emitter = amm.emit();
        emitters.push(emitter);
    }

    Ok(tokio::spawn(async move {
        for emitter in emitters {
            emitter.join().unwrap();
        }
    }))
}
