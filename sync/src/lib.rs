
mod uniswap_v2;
mod uniswap_v3;
mod types;
mod events;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use bincode::{ Decode, Encode};
use tokio::task::JoinHandle;
use tokio::sync::{RwLock};
use crate::uniswap_v2::UniswapV2Metadata;
use crate::uniswap_v3::UniswapV3Metadata;
use crate::events::EventEmitter;
use async_std::sync::Arc;
use async_trait::async_trait;
use std::hash::*;
use std::fmt::Display;
#[async_trait]
pub trait LiquidityProvider: EventEmitter {
	type Metadata;
	fn get_metadata(&self) -> Self::Metadata;
	async fn get_pools(&self) -> HashMap<String, Pool>;
	fn load_pools(&self) -> JoinHandle<()>;
	fn get_id(&self) -> LiquidityProviders;
}

#[derive( Decode, Encode, Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LiquidityProviders {
	UniswapV2,
	UniswapV3
}

impl From<u8> for LiquidityProviders {
	fn from(value: u8) -> Self {
		match value {
			1 => LiquidityProviders::UniswapV2,
			2 => LiquidityProviders::UniswapV3,
			_ => panic!("Invalid liquidity provider"),
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
	pub fn build_calculator(&self) -> Box<dyn Calculator> {
		match *self {
			LiquidityProviders::UniswapV2 => Box::new(CpmmCalculator::new()),
			LiquidityProviders::UniswapV3 => Box::new(CpmmCalculator::new()),
			_ => panic!("Invalid liquidity provider"),
		}
	}
	pub fn build(&self) -> BoxedLiquidityProvider {
		match *self {
			LiquidityProviders::UniswapV2 => {
				let metadata = UniswapV2Metadata {

				};
				Box::new(uniswap_v2::UniSwapV2::new(metadata))
			}
			LiquidityProviders::UniswapV3 => {
				let metadata = UniswapV3Metadata {

				};
				Box::new(uniswap_v3::UniSwapV3::new(metadata))
			}
			_ => panic!("Invalid liquidity provider"),
		}
	}
}


#[derive( Decode, Encode,Debug, Clone, Hash, Eq, PartialEq)]
pub enum Curve {
	Uncorrelated,
	Stable
}

#[derive( Decode, Encode, Debug, Clone, Eq)]
pub struct Pool {
	pub address: String,
	pub x_address: String,
	pub y_address: String,
	pub curve: Option<String>,
	pub curve_type: Curve,
	pub fee_bps: u64,
	pub x_amount: u64,
	pub y_amount: u64,
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
		self.address == other.address && self.x_address == other.x_address && self.y_address == other.y_address && self.provider == other.provider && self.curve_type == other.curve_type
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

impl Calculator for CpmmCalculator {
	fn calculate_out(&self, in_: u64, pool: &Pool) -> u64 {
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
			return swap_destination_amount;
		}
		let amount_in_with_fee = (in_ as u128) * ((10000 - pool.fee_bps) as u128);
		let numerator = amount_in_with_fee
			  .checked_mul(swap_destination_amount as u128)
			  .unwrap_or(0);
		let denominator = ((swap_source_amount as u128) * 10000) + amount_in_with_fee;
		(numerator / denominator) as u64
	}
}

pub trait Calculator {
	fn calculate_out(&self, in_: u64, pool: &Pool) -> u64;
}
pub struct CpmmCalculator {}

pub trait Meta {}


pub struct SyncConfig {
	pub providers: Vec<LiquidityProviders>,
}



pub async fn start(
	pools: Arc<RwLock<HashMap<String, Pool>>>,
	config: SyncConfig,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
	
	let mut join_handles = vec![];
	let mut amms = vec![];
	for provider in config.providers {
		let mut amm = provider.build();
		join_handles.push(amm.load_pools());
		amms.push(amm);
	}
	
	// Make sure pools are loaded before starting the listener
	println!("Loading Pools...");
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
	
	Ok(tokio::spawn(async move {
	
	}))
}
