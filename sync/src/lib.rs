#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

#[macro_use]
extern crate serde_derive;
extern crate rmp_serde as rmps;
mod abi;
mod events;
pub mod types;
pub mod uniswap_v2;
pub mod balancer_weighted;
pub mod uniswap_v3;
pub mod sushiswap;
pub mod node_dispatcher;
use crate::events::EventEmitter;
use crate::uniswap_v2::UniswapV2Metadata;
use crate::uniswap_v3::UniswapV3Metadata;
use crate::balancer_weighted::BalancerWeigtedMetadata;
use async_std::sync::Arc;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use coingecko::response::coins::CoinsMarketItem;
use ethers::types::{H160, H256, I256, Transaction, U256};
use ethers_providers::{Provider, Ws};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::*;
use std::str::FromStr;
use std::time::Duration;
use anyhow::Error;
use bincode::error::IntegerType::U128;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use once_cell::sync::Lazy;
pub static CHECKED_COIN: Lazy<String> = Lazy::new(|| {
    std::env::var("ETH_CHECKED_COIN")
          .unwrap_or("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".to_string())
});

pub const POLL_INTERVAL: u64 = 600;
use nom::FindSubstring;
use num_bigfloat::{BigFloat, RoundingMode};
use tracing::info;
use crate::node_dispatcher::NodeDispatcher;
use crate::sushiswap::SushiSwapMetadata;
use tokio::runtime::Runtime;
use rmps::{Deserializer, Serializer};


#[async_trait]
pub trait LiquidityProvider: EventEmitter<Box<dyn EventSource<Event = PoolUpdateEvent>>> + EventEmitter<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>> {
    type Metadata;
    fn get_metadata(&self) -> Self::Metadata;
    async fn get_pools(&self) -> HashMap<String, Pool>;
    async fn set_pools(&self, pools:HashMap<String, Pool>);
    fn set_update_pools(&mut self, pools:Vec<[Arc<RwLock<Pool>>; 2]>);
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()>;
    fn get_id(&self) -> LiquidityProviderId;
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LiquidityProviders {
    UniswapV2(UniswapV2Metadata),
    UniswapV3(UniswapV3Metadata),
    SushiSwap(UniswapV2Metadata),
    BalancerWeighted(BalancerWeigtedMetadata),
    Solidly(UniswapV2Metadata),
    Pancakeswap(UniswapV2Metadata)
}

#[derive(Deserialize, Serialize,Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
pub enum LiquidityProviderId {
    UniswapV2,
    UniswapV3,
    SushiSwap,
    BalancerWeighted,
    Solidly,
    Pancakeswap
}

impl<T: Into<String>> From<T> for LiquidityProviders {
    fn from(value: T) -> Self {
        match value.into().as_str() {
            "1" => LiquidityProviders::UniswapV2(Default::default()),
            "2" => LiquidityProviders::UniswapV3(UniswapV3Metadata {
                sqrt_price: U256::zero(),
                ..Default::default()
            }),
            "3" => LiquidityProviders::SushiSwap(Default::default()),
            "4" => LiquidityProviders::BalancerWeighted(Default::default()),
            "5" => LiquidityProviders::Solidly(Default::default()),
            "6" => LiquidityProviders::Pancakeswap(Default::default()),
            val => match val.find_substring("UniswapV2") {
                Some(v) => serde_json::from_str(val).unwrap(),
                None => {
                    match val.find_substring("UniswapV3") {
                        Some(v) => serde_json::from_str(val).unwrap(),
                        None => {
                            match val.find_substring("SushiSwap") {
                                Some(v) => serde_json::from_str(val).unwrap(),
                                None => {
                                    match val.find_substring("BalancerWeighted") {
                                        Some(v) => {
                                            serde_json::from_str(val).unwrap()
                                        },
                                        None => match val.find_substring("Solidly") {
                                            Some(v) => {
                                                serde_json::from_str(val).unwrap()
                                            },
                                            None => match val.find_substring("Pancake") {
                                                Some(v) => {
                                                    serde_json::from_str(val).unwrap()
                                                },
                                                None => panic!("Invalid Liquidity Provider {}", val)
                                            }
                                        }
                                    }
                                },
                            }
                        },
                    }
                }
            },
        }
    }
}

impl From<LiquidityProviders> for u8 {
    fn from(value: LiquidityProviders) -> Self {
        match value {
            LiquidityProviders::UniswapV2(_) => 1,
            LiquidityProviders::UniswapV3(_) => 2,
            LiquidityProviders::SushiSwap(_) => 3,
            LiquidityProviders::BalancerWeighted(_) => 4,
            LiquidityProviders::Solidly(_) => 5,
            LiquidityProviders::Pancakeswap(_) => 6,
        }
    }
}
pub trait EventSource: Send {
    type Event: Send;
    fn get_event(&self) -> Self::Event;
}
pub type BoxedLiquidityProvider = Box<
    dyn LiquidityProvider<Metadata = Box<dyn Meta>>
        + Send + Sync,
>;

impl LiquidityProviders {
    pub fn build_calculator(&self) -> Box<dyn Calculator> {
        match self {
            LiquidityProviders::UniswapV2(meta) => Box::new(CpmmCalculator::new()),
            LiquidityProviders::Solidly(meta) => Box::new(SolidlyCalculator::new()),
            LiquidityProviders::Pancakeswap(meta) => Box::new(PancakeCalculator::new()),
            LiquidityProviders::UniswapV3(meta) => {
                Box::new(UniswapV3Calculator::new(meta.clone()))
            }
            LiquidityProviders::SushiSwap(meta) => Box::new(CpmmCalculator::new()),
            LiquidityProviders::BalancerWeighted(meta) => Box::new(BalancerWeightedCalculator::new(meta.clone())),

            _ => panic!("Invalid liquidity provider"),
        }
    }
    pub fn build(&self, node_providers: NodeDispatcher) -> BoxedLiquidityProvider {
        match self {
            LiquidityProviders::UniswapV2(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(metadata, node_providers, LiquidityProviderId::UniswapV2))
            }
             LiquidityProviders::Solidly(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x777de5Fe8117cAAA7B44f396E93a401Cf5c9D4d6".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(metadata, node_providers, LiquidityProviderId::Solidly))
            }
            LiquidityProviders::Pancakeswap(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x1097053Fd2ea711dad45caCcc45EfF7548fCB362".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(metadata, node_providers, LiquidityProviderId::Pancakeswap))
            }
            LiquidityProviders::UniswapV3(meta) => {
                let metadata = UniswapV3Metadata {
                    factory_address: "0x1F98431c8aD98523631AE4a59f267346ea31F984".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v3::UniSwapV3::new(metadata, node_providers))
            }
            LiquidityProviders::SushiSwap(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac".to_string(),
                    ..meta.clone()
                };
                Box::new(sushiswap::SushiSwap::new(metadata, node_providers))
            }
            LiquidityProviders::BalancerWeighted(meta) => {
                let metadata = BalancerWeigtedMetadata {
                    factory_address: "0x8E9aa87E45e92bad84D5F8DD1bff34Fb92637dE9".to_string(),
                    ..meta.clone()
                };
                Box::new(balancer_weighted::BalancerWeighted::new(metadata, node_providers))
            }
            _ => panic!("Invalid liquidity provider"),
        }
    }
    
    pub fn id(&self) -> LiquidityProviderId{
        match self {
            LiquidityProviders::UniswapV2(_) => LiquidityProviderId::UniswapV2,
            LiquidityProviders::UniswapV3(_) => LiquidityProviderId::UniswapV3,
            LiquidityProviders::SushiSwap(_) => LiquidityProviderId::SushiSwap,
            LiquidityProviders::BalancerWeighted(_) => LiquidityProviderId::BalancerWeighted,
            LiquidityProviders::Solidly(_) => LiquidityProviderId::Solidly,
            LiquidityProviders::Pancakeswap(_) => LiquidityProviderId::Pancakeswap,

            
        }
    }

    pub fn factory_address(&self) -> String {
        match self {
            LiquidityProviders::UniswapV2(meta) => meta.factory_address.clone(),
            LiquidityProviders::UniswapV3(meta) => meta.factory_address.clone(),
            LiquidityProviders::SushiSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::BalancerWeighted(meta) => meta.factory_address.clone(),
            LiquidityProviders::Solidly(meta) => meta.factory_address.clone(),
            LiquidityProviders::Pancakeswap(meta) => meta.factory_address.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub enum Curve {
    Uncorrelated,
    Stable,
}

pub trait PoolInfo {
    fn getX(&self) -> String;
    fn getY(&self) -> String;
    fn supports_callback_payment(&self) -> bool;
    fn supports_pre_payment(&self) -> bool;
}



#[derive(Serialize, Deserialize, Debug, Clone, Eq)]
pub struct Pool {
    pub address: String,
    pub x_address: String,
    pub y_address: String,
    pub curve: Option<String>,
    pub curve_type: Curve,
    pub fee_bps: u64,
    pub x_amount: U256,
    pub y_amount: U256,
    pub x_to_y: bool,
    pub provider: LiquidityProviders,
}

impl PoolInfo for Pool {
    fn getX(&self) -> String {
        self.x_address.clone()
    }
    
    fn getY(&self) -> String {
        self.y_address.clone()
    }

    fn supports_callback_payment(&self) -> bool {
        match self.provider.id() {
            LiquidityProviderId::UniswapV2 => true,
            LiquidityProviderId::Solidly => true,
            LiquidityProviderId::Pancakeswap => true,
            LiquidityProviderId::UniswapV3 => true,
            LiquidityProviderId::SushiSwap=> true,
            LiquidityProviderId::BalancerWeighted=> false,
        }
    }

    fn supports_pre_payment(&self) -> bool {
        match self.provider.id() {
            LiquidityProviderId::UniswapV2 => false,
            LiquidityProviderId::Solidly => false,
            LiquidityProviderId::Pancakeswap => false,
            LiquidityProviderId::UniswapV3 => false,
            LiquidityProviderId::SushiSwap => false,
            LiquidityProviderId::BalancerWeighted => false,
        }
    }
}

impl Hash for Pool {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.address.hash(state);
        self.x_address.hash(state);
        self.y_address.hash(state);
    }
}

impl PartialEq for Pool {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
            && self.x_address == other.x_address
            && self.y_address == other.y_address
    }
}
impl EventSource for Pool {
    type Event = Self;
    fn get_event(&self) -> Self::Event {
        self.clone()
    }
}

#[derive(Debug, Clone, PartialEq,Eq)]
pub struct PoolUpdateEvent {
    pub pool: Pool,
    pub block_number: u64,
    pub timestamp: u128
}

#[derive(Debug, Clone, PartialEq,Eq)]
pub struct PendingPoolUpdateEvent {
    pub pool: Pool,
    pub pending_tx: Transaction,
    pub timestamp: u128,
    pub block_number: u64
}

impl EventSource for PoolUpdateEvent {
    type Event = Self;
    fn get_event(&self) -> Self::Event {
        self.clone()
    }
}
impl EventSource for PendingPoolUpdateEvent {
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
        write!(f, "Pool {{ \n\tProvider: {:?}\n\taddress: {}\n\tx_address: {}\n\ty_address: {}\n\tis_x_to_y: {}\n\t", self.provider.id(), self.address,self.x_address, self.y_address,  self.x_to_y)
    }
}

impl CpmmCalculator {
    fn new() -> Self {
        Self {}
    }
}

impl Calculator for CpmmCalculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero() {
            return Err(Error::msg("Insufficient Liquidity"))
        }
        let amount_in_with_fee = in_.saturating_mul(U256::from(10000 - 30));
        let numerator = amount_in_with_fee
            .checked_mul(swap_destination_amount)
            .unwrap_or(U256::from(0));
        let denominator = ((swap_source_amount) * 10000) + amount_in_with_fee;
        let amount_out = numerator / denominator;
        if amount_out > swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"))
        }

        Ok(amount_out)
    }
    
    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero() || out_ >= swap_destination_amount  {
            return Err(Error::msg("Insufficient Liquidity"))
        }


        if let Some(numerator) = swap_source_amount.checked_mul( out_ * 10000) {
            let denominator = (swap_destination_amount - out_) * U256::from((9970) as u128);
            Ok((numerator / denominator) + 1)

        } else {
            Err(Error::msg("Multiplication Overflow"))
        }

    }
}

pub trait Calculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256>;
    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256>;
}
pub struct CpmmCalculator {}
pub struct SolidlyCalculator {}
pub struct PancakeCalculator {}

impl SolidlyCalculator {
    pub fn new() -> Self {
        Self {}
    }
}

impl PancakeCalculator {
    pub fn new() -> Self {
        Self {}
    }
}
pub struct UniswapV3Calculator {
    meta: UniswapV3Metadata
}

pub struct BalancerWeightedCalculator {
    meta: BalancerWeigtedMetadata
}

impl BalancerWeightedCalculator {
    pub fn new(meta: BalancerWeigtedMetadata) -> Self {
        Self {
            meta
        }
    }

    // TODO
    pub fn mul_up(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        (x * y).round(18, RoundingMode::Up)

    }

    pub fn mul_down(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        (x * y).round(18, RoundingMode::Down)
    }

    pub fn div_up(&self, x: BigFloat, y: BigFloat) -> BigFloat {

        (x/y).round(18, RoundingMode::Up)
    }
    pub fn div_down(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        (x/y).round(18, RoundingMode::Down)
    }

    pub fn pow_up(&self, x: BigFloat, y: BigFloat) -> BigFloat {

        if y == BigFloat::from(1) {
            x
        } else if y == BigFloat::from(2) {
            self.mul_up(x, x)
        } else if y == BigFloat::from(4) {
            let square = self.mul_up(x, x);
            self.mul_up(square, square)
        } else {
             x.pow(&y)
        }


    }
}

impl Calculator for SolidlyCalculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero() {
            return Err(Error::msg("Insufficient Liquidity"))
        }
        let amount_in_with_fee = in_.saturating_mul(U256::from(10000 - 20));
        let numerator = amount_in_with_fee
            .checked_mul(swap_destination_amount)
            .unwrap_or(U256::from(0));
        let denominator = ((swap_source_amount) * 10000) + amount_in_with_fee;
        let amount_out = numerator / denominator;
        if amount_out > swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"))
        }

        Ok(amount_out)
    }

    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero() || out_ >= swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"))
        }

        if let Some(numerator) = swap_source_amount.checked_mul( out_ * 10000) {
            let denominator = (swap_destination_amount - out_) * U256::from((9980) as u128);
            Ok((numerator / denominator) + 1)

        } else {
            Err(Error::msg("Multiplication Overflow"))
        }

    }
}


impl Calculator for PancakeCalculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero() {
            return Err(Error::msg("Insufficient Liquidity"))
        }
        let amount_in_with_fee = in_.saturating_mul(U256::from(10000 - 25));
        let numerator = amount_in_with_fee
            .checked_mul(swap_destination_amount)
            .unwrap_or(U256::from(0));
        let denominator = ((swap_source_amount) * 10000) + amount_in_with_fee;
        let amount_out = numerator / denominator;
        if amount_out > swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"))
        }

        Ok(amount_out)
    }

    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero()  || out_ >= swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"))
        }

        if let Some(numerator) = swap_source_amount.checked_mul( out_ * 10000) {
            let denominator = (swap_destination_amount - out_) * U256::from((9975) as u128);
            Ok((numerator / denominator) + 1)

        } else {
            Err(Error::msg("Multiplication Overflow"))
        }

    }
}


impl UniswapV3Calculator {
    pub const MAX_SQRT_RATIO: &str = "1461446703485210103287273052203988822378723970342";
    pub const MIN_SQRT_RATIO: &str = "4295128739";
    pub fn new(meta: UniswapV3Metadata) -> Self {
        Self {
            meta,
        }
    }
}
#[async_trait]
impl Calculator for UniswapV3Calculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if pool.y_amount == U256::from(0) || pool.x_amount == U256::from(0) {
            return Err(Error::msg("Insufficient Liquidity"))
        }
        let amount_out = self.meta.simulate_swap(pool.x_to_y, in_, true)?;
        if amount_out > swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"))
        }
                Ok(amount_out)
        }
    
    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };
        if pool.y_amount == U256::from(0) || pool.x_amount == U256::from(0) || out_ >= swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"))
        }

        let amount_in = self.meta.simulate_swap(pool.x_to_y, out_, false)?;
        Ok(amount_in)

    }
    
    
    
    
    }



#[async_trait]
impl Calculator for BalancerWeightedCalculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (source_token, dest_token) = if pool.x_to_y {
            (self.meta.tokens.first().unwrap(), self.meta.tokens.last().unwrap())
        } else {
            (self.meta.tokens.last().unwrap(), self.meta.tokens.first().unwrap())
        };
        if pool.y_amount == U256::from(0) || pool.x_amount == U256::from(0) {
            return Err(Error::msg("Insufficient Liquidity"))
        }

        let bo = BigFloat::from_str(&dest_token.balance.to_string()).unwrap();
        let bi = BigFloat::from_str(&source_token.balance.to_string()).unwrap();
        let ai = BigFloat::from_str(&in_.to_string()).unwrap();
        let wo = BigFloat::from_str(&dest_token.weight.to_string()).unwrap();
        let wi = BigFloat::from_str(&source_token.weight.to_string()).unwrap();


        let denominator = bi + ai;
        let base = self.div_up(bi, denominator);
        let exponent = self.div_down(wi, wo);
        let power = self.pow_up(base,exponent);
        let amount_out = self.mul_down(bo, power.sub(&BigFloat::from(1)));
        let amount_out_with_fee = self.div_up(
            amount_out,
            BigFloat::from(1).sub(&BigFloat::from_str(&self.meta.swap_fee.to_string()).unwrap().div(&BigFloat::from(10).pow(&BigFloat::from(18)))));

        if let Some(amount_out_uint) = amount_out_with_fee.to_u128() {
            Ok(U256::from(amount_out_uint))

        } else {
            Err(Error::msg("Casting Error"))

        }
    }

    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (source_token, dest_token) = if pool.x_to_y {
            (self.meta.tokens.first().unwrap(), self.meta.tokens.last().unwrap())
        } else {
            (self.meta.tokens.last().unwrap(), self.meta.tokens.first().unwrap())
        };

        if pool.y_amount == U256::from(0) || pool.x_amount == U256::from(0) || out_ >= dest_token.balance {
            return Err(Error::msg("Insufficient Liquidity"))
        }


        let bo = BigFloat::from_str(&dest_token.balance.to_string()).unwrap();
        let bi = BigFloat::from_str(&source_token.balance.to_string()).unwrap();
        let ao = BigFloat::from_str(&out_.to_string()).unwrap();
        let wo = BigFloat::from_str(&dest_token.weight.to_string()).unwrap();
        let wi = BigFloat::from_str(&source_token.weight.to_string()).unwrap();


        let base = self.div_up(bo, bo.sub(&ao));
        let exponent = self.div_up(wo, wi);
        let power = self.pow_up(base, exponent);
        // info!("bo: {} bi: {} ao: {} base: {} exponent: {} power: {}", bo.to_u128().unwrap(), bi.to_u128().unwrap(), ao.to_u128().unwrap(), base, exponent, power);
        let amount_in = self.mul_up(bi, power.sub(&BigFloat::from(1)));
        let amount_in_with_fee = self.div_up(
            amount_in,
            BigFloat::from(1).sub(&BigFloat::from_str(&self.meta.swap_fee.to_string()).unwrap().div(&BigFloat::from(10).pow(&BigFloat::from(18)))));
        if let Some(amount_in_uint) = amount_in_with_fee.to_u128() {
            Ok(U256::from(amount_in_uint))

        } else {
            Err(Error::msg("Casting Error"))

        }

    }




}
pub trait Meta {}

pub struct SyncConfig {
    pub providers: Vec<LiquidityProviders>,
    pub from_file: bool
}

pub async fn start(
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    updated_q: kanal::AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>,
    pending_updated_q: kanal::AsyncSender<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>,
    mut used_pools: tokio::sync::oneshot::Receiver<Vec<[Arc<RwLock<Pool>>; 2]>>,
    nodes: NodeDispatcher,
    config: SyncConfig,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {


    let mut join_handles = vec![];
    let mut amms = vec![];
    for provider in config.providers {
        let mut amm = provider.build(nodes.clone());
        if !config.from_file {
            join_handles.push(amm.load_pools(vec![]));
        }
        amm.subscribe(updated_q.clone());
        amm.subscribe(pending_updated_q.clone());
        amms.push(amm);
    }

    let filter_tokens: Vec<String> = serde_json::from_str(&std::fs::read_to_string("blacklisted_tokens.json").unwrap()).unwrap();
    let filter_pools: Vec<String> = serde_json::from_str(&std::fs::read_to_string("blacklisted_pools.json").unwrap()).unwrap();
    // Make sure pools are loaded before starting the listener
    info!("Loading Pools... ");
    for handle in join_handles {
        handle.await?;
    }
    let mut loaded_pools = HashMap::new();
    for amm in &amms {
        let pools = amm.get_pools().await;
        let keys: Vec<String> = pools.keys().cloned().collect();
        std::fs::write(format!("{:?}", amm.get_id()), keys.join("\n")).expect("");
        for (addr, pool) in pools {
            if filter_tokens.contains(&pool.x_address) || filter_tokens.contains(&pool.y_address) {
                continue;
            }
            if filter_pools.contains(&pool.address) {
                continue;
            }
            loaded_pools.insert(addr, pool);
        }
    }

    let mut pools = pools.write().await;
    *pools = loaded_pools;
    std::mem::drop(pools);



    Ok(tokio::spawn(async move {
        let rt = Runtime::new().unwrap();

        let used_pools = used_pools.await.unwrap();
        let mut emitters = vec![];

        for mut amm in amms {
            let mut my_update_pools = vec![];
            let mut my_pools = HashMap::new();
            for pools in &used_pools {
                let pool = pools[0].read().await;
                if pool.provider.id() == amm.get_id() {
                    my_pools.insert(pool.address.clone(), pool.clone());
                    my_update_pools.push(pools.clone());
                }
            }
                amm.set_update_pools(my_update_pools);
                amm.set_pools(my_pools).await;
                emitters.push(EventEmitter::<Box<dyn EventSource<Event=PoolUpdateEvent>>>::emit(&*amm));
                emitters.push(EventEmitter::<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>::emit(&*amm));
            }
            for emitter in emitters {
                emitter.join().unwrap();
            }
    }))
}

#[cfg(test)]
mod tests {
    use ethers::providers::{Provider, Ws};
    use tokio::test;
    #[test]
    async fn test_uniswap_v3_calculator() {
        let mut meta = UniswapV3Metadata {
            factory_address: "".to_string(),
            address: "0x88e6a0c2ddd26feeb64f039a2c41296fcb3f5640".to_string(),
            token_a: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(),
            token_b: "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".to_string(),
            token_a_decimals: 6,
            token_b_decimals: 18,
            fee: 500,
            liquidity: 229553535761302309707,
            sqrt_price: "2030685815623659403894576284118504".to_string(),
            tick: 203048,
            tick_spacing: 10,
            liquidity_net: 0,
            ..Default::default()
        };
        let eth_client = Arc::new(
            Provider::<Ws>::connect("ws://65.21.198.115:8546")
                .await
                .unwrap(),
        );
        let x_y = abi::uniswap_v3::get_uniswap_v3_tick_data_batch_request(&meta, meta.tick, true, 160, None, eth_client.clone()).await.unwrap();
        let y_x = abi::uniswap_v3::get_uniswap_v3_tick_data_batch_request(&meta, meta.tick, false, 160, None, eth_client.clone()).await.unwrap();
        meta.tick_bitmap_x_y = x_y;
        meta.tick_bitmap_y_x = y_x;
        let pool = Pool {
            address: "".to_string(),
            x_address: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48 ".to_string(),
            y_address: "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2 ".to_string(),
            curve: None,
            curve_type: Curve::Uncorrelated,
            fee_bps: 0,
            x_amount: 0,
            y_amount: 0,
            x_to_y: true,
            provider: LiquidityProviders::UniswapV3(meta.clone())
        };
    


        let calculator = pool.provider.build_calculator();
        
        let out_ = calculator.calculate_out(U256::from(10_u128.pow(8)) , &pool).unwrap();
        let in_ = calculator.calculate_in(out_ , &pool).unwrap();
        println!("{} {}", in_, out_);

    }
    
    
}