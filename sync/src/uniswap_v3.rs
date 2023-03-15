#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use crate::abi::{uniswap_v3::SwapFilter,IERC20};
use crate::types::UniSwapV3Pool;
use crate::types::UniSwapV3Token;
use crate::{LiquidityProviderId, Meta, PoolUpdateEvent, UniswapV3Calculator};
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
use serde::{Deserialize, Serialize};
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
use tracing::{info, debug, trace, error};
use crate::abi::uniswap_v3::{get_complete_pool_data_batch_request, get_uniswap_v3_tick_data_batch_request, UniswapV3TickData};
use crate::node_dispatcher::NodeDispatcher;

const TVL_FILTER_LEVEL: i32 = 1;
// Todo: add word in here to update and remove middleware use in simulate_swap
#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct UniswapV3Metadata {
    pub factory_address: String,
    pub address: String,
    pub token_a: String,
    pub token_b: String,
    pub token_a_amount: U256,
    pub token_b_amount: U256,
    pub token_a_decimals: u8,
    pub token_b_decimals: u8,
    pub fee: u32,
    pub liquidity: U256,
    pub sqrt_price: U256,
    pub tick: i32,
    pub tick_spacing: i32,
    pub liquidity_net: I256,
    pub tick_bitmap_x_y: Vec<UniswapV3TickData>,
    pub tick_bitmap_y_x: Vec<UniswapV3TickData>,
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

#[derive(Debug)]
pub struct CurrentState {
    amount_specified_remaining: I256,
    amount_calculated: I256,
    sqrt_price_x_96: U256,
    tick: i32,
    liquidity: U256,
}
impl UniswapV3Metadata {
    pub fn calculate_virtual_reserves(&self) -> (u128, u128) {
        let sqrt_price = self.sqrt_price;
        let price = BigFloat::from_u128(
            (U256::from(sqrt_price.overflowing_mul(sqrt_price).0) >> 128).as_u128(),
        )
        .div(&BigFloat::from(2f64.powf(64.0)))
        .mul(&BigFloat::from_f64(10f64.powf(
            (self.token_a_decimals as i8 - self.token_b_decimals as i8) as f64,
        )));

        let sqrt_price = price.sqrt();
        let liquidity = BigFloat::from_str(&self.liquidity.to_string()).unwrap();

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
        let sqrt_price =self.sqrt_price;
    
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
    
    pub fn calculate_compressed(&self, tick: i32) -> i32 {
        if tick < 0 && tick % self.tick_spacing != 0 {
            (tick / self.tick_spacing) - 1
        } else {
            tick / self.tick_spacing
        }
    }
    
    pub fn calculate_word_pos_bit_pos(&self, compressed: i32) -> (i16, u8) {
        uniswap_v3_math::tick_bit_map::position(compressed)
    }
    
    pub async fn get_word(
        &self,
        word_pos: i16,
        block_number: Option<U64>,
        middleware: Arc<Provider<Ws>>,
    ) -> anyhow::Result<U256> {
        if block_number.is_some() {
            //TODO: in the future, create a batch call to get this and liquidity net within the same call
            Ok(
                crate::abi::uniswap_v3::UniswapV3Pool::new(H160::from_str(&self.address)?, middleware.clone())
                      .tick_bitmap(word_pos)
                      .block(block_number.unwrap())
                      .call()
                      .await?,
            )
        } else {
            //TODO: in the future, create a batch call to get this and liquidity net within the same call
            Ok(
                crate::abi::uniswap_v3::UniswapV3Pool::new(H160::from_str(&self.address)?, middleware.clone())
                      .tick_bitmap(word_pos)
                      .call()
                      .await?,
            )
        }
    }
    
    pub async fn get_tick_info(
        &self,
        tick: i32,
        middleware: Arc<Provider<Ws>>,
    ) -> anyhow::Result<(u128, i128, U256, U256, i64, U256, u32, bool)> {
        let v3_pool =
              crate::abi::uniswap_v3::UniswapV3Pool::new(H160::from_str(&self.address)?, middleware.clone());
        
        let tick_info = v3_pool.ticks(tick).call().await?;
        
        Ok((
            tick_info.0,
            tick_info.1,
            tick_info.2,
            tick_info.3,
            tick_info.4,
            tick_info.5,
            tick_info.6,
            tick_info.7,
        ))
    }
    
    pub async fn get_liquidity_net(
        &self,
        tick: i32,
        middleware: Arc<Provider<Ws>>,
    ) -> anyhow::Result<i128> {
        let tick_info = self.get_tick_info(tick, middleware).await?;
        Ok(tick_info.1)
    }
    
    pub fn simulate_swap(
        &self,
        zero_for_one: bool,
        amount_in: U256,
        exact_in: bool
    ) -> anyhow::Result<U256> {
        if amount_in.is_zero() {
            return Ok(U256::zero());
        }

        let mut amount_in = I256::from_raw(amount_in);

        if !exact_in {
            amount_in = -amount_in;
        }





        //TODO: make this a queue instead of vec and then an iterator FIXME::
        let mut tick_data = if zero_for_one {
            self.tick_bitmap_x_y.clone()
        } else {
            self.tick_bitmap_y_x.clone()
        };

        let mut tick_data_iter = tick_data.iter();

        //Set sqrt_price_limit_x_96 to the max or min sqrt price in the pool depending on zero_for_one
        let sqrt_price_limit_x_96 = if zero_for_one {
            MIN_SQRT_RATIO + 1
        } else {
            MAX_SQRT_RATIO - 1
        };

        //Initialize a mutable state state struct to hold the dynamic simulated state of the pool
        let mut current_state = CurrentState {
            sqrt_price_x_96: self.sqrt_price, //Active price on the pool
            amount_calculated: I256::zero(),  //Amount of token_out that has been calculated
            amount_specified_remaining: amount_in, //Amount of token_in that has not been swapped
            tick: self.tick,                                       //Current i24 tick of the pool
            liquidity: self.liquidity, //Current available liquidity in the tick range
        };

        while current_state.amount_specified_remaining != I256::zero()
            && current_state.sqrt_price_x_96 != sqrt_price_limit_x_96
        {
            //Initialize a new step struct to hold the dynamic state of the pool at each step
            let mut step = StepComputations {
                sqrt_price_start_x_96: current_state.sqrt_price_x_96, //Set the sqrt_price_start_x_96 to the current sqrt_price_x_96
                ..Default::default()
            };

            let next_tick_data = if let Some(tick_data) = tick_data_iter.next() {
                tick_data
            } else {

                    //This should never happen, but if it does, we should return an error because something is wrong
                    return Err(anyhow::Error::msg("UniswapV3Calculator: Out of tick liquidity"));

            };

            step.tick_next = next_tick_data.tick;


            //Get the next sqrt price from the input amount
            step.sqrt_price_next_x96 =
                uniswap_v3_math::tick_math::get_sqrt_ratio_at_tick(step.tick_next)?;

            //Target spot price
            let swap_target_sqrt_ratio = if zero_for_one {
                if step.sqrt_price_next_x96 < sqrt_price_limit_x_96 {
                    sqrt_price_limit_x_96
                } else {
                    step.sqrt_price_next_x96
                }
            } else if step.sqrt_price_next_x96 > sqrt_price_limit_x_96 {
                sqrt_price_limit_x_96
            } else {
                step.sqrt_price_next_x96
            };

            //Compute swap step and update the current state
            (
                current_state.sqrt_price_x_96,
                step.amount_in,
                step.amount_out,
                step.fee_amount,
            ) = uniswap_v3_math::swap_math::compute_swap_step(
                current_state.sqrt_price_x_96,
                swap_target_sqrt_ratio,
                current_state.liquidity.as_u128(),
                current_state.amount_specified_remaining,
                self.fee,
            )?;
            if exact_in {
                current_state.amount_specified_remaining = current_state
                    .amount_specified_remaining
                    .overflowing_sub(I256::from_raw(
                        step.amount_in.overflowing_add(step.fee_amount).0,
                    ))
                    .0;

                current_state.amount_calculated -= I256::from_raw(step.amount_out);
            } else {
                current_state.amount_specified_remaining += I256::from_raw(step.amount_out);
                current_state.amount_calculated = current_state
                    .amount_calculated.overflowing_add(I256::from_raw(
                    step.amount_in.overflowing_add(step.fee_amount).0,
                ))
                    .0;
            }

            //If the price moved all the way to the next price, recompute the liquidity change for the next iteration
            if current_state.sqrt_price_x_96 == step.sqrt_price_next_x96 {
                if next_tick_data.initialized {
                    let mut liquidity_net = next_tick_data.liquidity_net;

                    // we are on a tick boundary, and the next tick is initialized, so we must charge a protocol fee
                    if zero_for_one {
                        liquidity_net = -liquidity_net;
                    }

                    current_state.liquidity = if liquidity_net < I256::zero() {
                        current_state.liquidity.checked_sub((-liquidity_net).into_raw()).unwrap_or(U256::from(0))
                    } else {
                        current_state.liquidity + (liquidity_net.into_raw())
                    };

                    //Increment the current tick
                    current_state.tick = if zero_for_one {
                        step.tick_next.wrapping_sub(1)
                    } else {
                        step.tick_next
                    }
                }
                //If the current_state sqrt price is not equal to the step sqrt price, then we are not on the same tick.
                //Update the current_state.tick to the tick at the current_state.sqrt_price_x_96
            } else if current_state.sqrt_price_x_96 != step.sqrt_price_start_x_96 {
                current_state.tick = uniswap_v3_math::tick_math::get_tick_at_sqrt_ratio(
                    current_state.sqrt_price_x_96,
                )?;
            }
        }

        let amount = if current_state.amount_calculated < I256::from(0) {
            (-current_state.amount_calculated).into_raw()
        } else {
            current_state.amount_calculated.into_raw()
        };

        Ok(amount)
    }


    pub fn simulate_swap_mut(
        &mut self,
        zero_for_one: bool,
        amount_in: U256,
        exact_in: bool
    ) -> anyhow::Result<U256> {
        if amount_in.is_zero() {
            return Ok(U256::zero());
        }

        let mut amount_in = I256::from_raw(amount_in);

        if !exact_in {
            amount_in = -amount_in;
        }

        //TODO: make this a queue instead of vec and then an iterator FIXME::
        let mut tick_data = if zero_for_one {
            self.tick_bitmap_x_y.clone()
        } else {
            self.tick_bitmap_y_x.clone()
        };

        let mut tick_data_iter = tick_data.iter();

        //Set sqrt_price_limit_x_96 to the max or min sqrt price in the pool depending on zero_for_one
        let sqrt_price_limit_x_96 = if zero_for_one {
            MIN_SQRT_RATIO + 1
        } else {
            MAX_SQRT_RATIO - 1
        };

        //Initialize a mutable state state struct to hold the dynamic simulated state of the pool
        let mut current_state = CurrentState {
            sqrt_price_x_96: self.sqrt_price, //Active price on the pool
            amount_calculated: I256::zero(),  //Amount of token_out that has been calculated
            amount_specified_remaining: amount_in, //Amount of token_in that has not been swapped
            tick: self.tick,                                       //Current i24 tick of the pool
            liquidity: self.liquidity, //Current available liquidity in the tick range
        };
        let mut liquidity_net = self.liquidity_net;
        while current_state.amount_specified_remaining != I256::zero()
            && current_state.sqrt_price_x_96 != sqrt_price_limit_x_96
        {
            //Initialize a new step struct to hold the dynamic state of the pool at each step
            let mut step = StepComputations {
                sqrt_price_start_x_96: current_state.sqrt_price_x_96, //Set the sqrt_price_start_x_96 to the current sqrt_price_x_96
                ..Default::default()
            };

            let next_tick_data = if let Some(tick_data) = tick_data_iter.next() {
                tick_data
            } else {

                //This should never happen, but if it does, we should return an error because something is wrong
                return Err(anyhow::Error::msg("UniswapV3Calculator: Out of tick liquidity"));

            };

            step.tick_next = next_tick_data.tick;


            //Get the next sqrt price from the input amount
            step.sqrt_price_next_x96 =
                uniswap_v3_math::tick_math::get_sqrt_ratio_at_tick(step.tick_next)?;

            //Target spot price
            let swap_target_sqrt_ratio = if zero_for_one {
                if step.sqrt_price_next_x96 < sqrt_price_limit_x_96 {
                    sqrt_price_limit_x_96
                } else {
                    step.sqrt_price_next_x96
                }
            } else if step.sqrt_price_next_x96 > sqrt_price_limit_x_96 {
                sqrt_price_limit_x_96
            } else {
                step.sqrt_price_next_x96
            };

            //Compute swap step and update the current state
            (
                current_state.sqrt_price_x_96,
                step.amount_in,
                step.amount_out,
                step.fee_amount,
            ) = uniswap_v3_math::swap_math::compute_swap_step(
                current_state.sqrt_price_x_96,
                swap_target_sqrt_ratio,
                current_state.liquidity.as_u128(),
                current_state.amount_specified_remaining,
                self.fee,
            )?;
            if exact_in {
                current_state.amount_specified_remaining = current_state
                    .amount_specified_remaining
                    .overflowing_sub(I256::from_raw(
                        step.amount_in.overflowing_add(step.fee_amount).0,
                    ))
                    .0;

                current_state.amount_calculated -= I256::from_raw(step.amount_out);
            } else {
                current_state.amount_specified_remaining += I256::from_raw(step.amount_out);
                current_state.amount_calculated = current_state
                    .amount_calculated.overflowing_add(I256::from_raw(
                    step.amount_in.overflowing_add(step.fee_amount).0,
                ))
                    .0;
            }

            //If the price moved all the way to the next price, recompute the liquidity change for the next iteration
            if current_state.sqrt_price_x_96 == step.sqrt_price_next_x96 {
                if next_tick_data.initialized {
                     liquidity_net = next_tick_data.liquidity_net;

                    // we are on a tick boundary, and the next tick is initialized, so we must charge a protocol fee
                    if zero_for_one {
                        liquidity_net = -liquidity_net;
                    }

                    current_state.liquidity = if liquidity_net < I256::zero() {
                        current_state.liquidity.checked_sub((-liquidity_net).into_raw()).unwrap_or(U256::from(0))
                    } else {
                        current_state.liquidity + (liquidity_net.into_raw())
                    };

                    //Increment the current tick
                    current_state.tick = if zero_for_one {
                        step.tick_next.wrapping_sub(1)
                    } else {
                        step.tick_next
                    }
                }
                //If the current_state sqrt price is not equal to the step sqrt price, then we are not on the same tick.
                //Update the current_state.tick to the tick at the current_state.sqrt_price_x_96
            } else if current_state.sqrt_price_x_96 != step.sqrt_price_start_x_96 {
                current_state.tick = uniswap_v3_math::tick_math::get_tick_at_sqrt_ratio(
                    current_state.sqrt_price_x_96,
                )?;
            }
        }

        let amount = if current_state.amount_calculated < I256::from(0) {
            (-current_state.amount_calculated).into_raw()
        } else {
            current_state.amount_calculated.into_raw()
        };

        self.liquidity = current_state.liquidity;
        self.sqrt_price = current_state.sqrt_price_x_96;
        self.tick = current_state.tick;
        self.liquidity_net = liquidity_net;
        Ok(amount)
    }
}

const UNISWAP_V3_DEPLOYMENT_BLOCK: u64 = 11969621;

pub const POOL_CREATED_EVENT_SIGNATURE: H256 = H256([
    120, 60, 202, 28, 4, 18, 221, 13, 105, 94, 120, 69, 104, 201, 109, 162, 233, 194, 47, 249, 137,
    53, 122, 46, 139, 29, 155, 43, 78, 107, 113, 24,
]);
pub struct UniSwapV3 {
    pub metadata: UniswapV3Metadata,
    pub pools: Arc<RwLock<HashMap<String, Pool>>>,
    subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>>>>,
    pending_subscribers: Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>>>>,
    nodes: NodeDispatcher
}

impl UniSwapV3 {
    pub fn new(metadata: UniswapV3Metadata, nodes: NodeDispatcher) -> Self {
        Self {
            metadata,
            pools: Arc::new(RwLock::new(HashMap::new())),
            subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            pending_subscribers: Arc::new(std::sync::RwLock::new(Vec::new())),
            nodes
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
// TODO: combine data and tick bitmap loader solidity script
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
        let node_url = self.nodes.next_free();
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let eth_client = Arc::new(
                Provider::<Ws>::connect(&node_url)
                    .await
                    .unwrap(),
            );
            let current_block = eth_client.get_block_number().await.unwrap().0[0];

            let step = 100000_u64;
            let mut handles = vec![];

            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(cores));
            let mut indices: Arc<Mutex<VecDeque<(u64, u64)>>> =
                Arc::new(Mutex::new(VecDeque::new()));

            for i in (UNISWAP_V3_DEPLOYMENT_BLOCK..current_block).step_by(step as usize) {
                let mut w = indices.lock().await;
                w.push_back((i, i + step));
            }

            loop {
                let permit = permits.clone().acquire_owned().await.unwrap();
                let mut w = indices.lock().await;
                if w.len() == 0 {
                    break;
                }
                let span = w.pop_back();
                drop(w);

                if let Some((from_block, to_block)) = span {
                    let eth_client = eth_client.clone();
                    let filter_tokens = filter_tokens.clone();
                    let pools = pools.clone();
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
                                .filter_map(|log| {

                                    let token_a = hex_to_address_string(log.topics.get(1).unwrap().encode_hex());
                                    let token_b = hex_to_address_string(log.topics.get(2).unwrap().encode_hex());
                                    let tokens = ethers::abi::decode(
                                        &[ParamType::Uint(32), ParamType::Address],
                                        &log.data,
                                    )
                                    .unwrap();
                                    match tokens.get(1).unwrap() {
                                        Token::Address(addr) => Some(addr.clone()),
                                        _ => Default::default(),
                                    }
                                })
                                .collect::<Vec<H160>>()
                                .as_slice()
                                .chunks(9)
                            {
                                let pairs_data =
                                    crate::abi::uniswap_v3::get_complete_pool_data_batch_request(
                                        chunk.to_vec(),
                                        eth_client.clone(),
                                    )
                                    .await;
                                if let Ok(mut pairs_data) = pairs_data {

                                    for meta in pairs_data {
                                        let min_0 = U256::from(10).pow(U256::from(meta.token_a_decimals as i32 + TVL_FILTER_LEVEL));
                                        let min_1 = U256::from(10).pow(U256::from(meta.token_b_decimals as i32 + TVL_FILTER_LEVEL));
                                        if meta.token_a_amount.lt(&min_0) || meta.token_b_amount.lt(&min_1)  {
                                            continue;
                                        }
                                        let pool = Pool {
                                            address: meta.address.clone(),
                                            x_address: meta.token_a.clone(),
                                            fee_bps: meta.fee as u64,
                                            y_address: meta.token_b.clone(),
                                            curve: None,
                                            curve_type: Curve::Uncorrelated,
                                            x_amount: meta.token_a_amount,
                                            y_amount: meta.token_b_amount,
                                            x_to_y: true,
                                            provider: LiquidityProviders::UniswapV3(meta),
                                        };

                                        let mut w = pools.write().await;
                                        w.insert(pool.address.clone(), pool);
                                    }
                                } else {
                                    info!("{:?}", pairs_data.unwrap_err())
                                }
                            }
                        } else {

                        }
                        drop(permit);
                    }));
                }
            }
            for handle in handles {
                handle.await;
            }


            info!(
                "{:?} Pools: {}",
                LiquidityProviderId::UniswapV3,
                pools.read().await.len()
            );
        })
    }
    fn get_id(&self) -> LiquidityProviderId {
        LiquidityProviderId::UniswapV3
    }
}
fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}
impl EventEmitter<Box<dyn EventSource<Event = PoolUpdateEvent>>> for UniSwapV3 {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>>>> {
        self.subscribers.clone()
    }
    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.pools.clone();
        let subscribers = self.subscribers.clone();
        let node_url = self.nodes.next_free();

        std::thread::spawn(move || {
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();
            rt.block_on( async move {
                let mut joins = vec![];
                let client = Arc::new(
                    Provider::<Ws>::connect(&node_url)
                          .await
                          .unwrap(),
                );
    
                let latest_block = client.get_block_number().await.unwrap();
    
                
                for pool in pools.read().await.values() {
                    let subscribers = subscribers.clone();
                    let mut pool = pool.clone();
                    let client = client.clone();
                    let subscribers = subscribers.read().unwrap();
                    let sub = subscribers.first().unwrap().clone();
                    drop(subscribers);

                    let pools = pools.clone();
                    joins.push(tokio::runtime::Handle::current().spawn(async move {
                        let event =
                              ethers::contract::Contract::event_of_type::<SwapFilter>(client.clone())
                                    .from_block(latest_block)
                                    .address(ValueOrArray::Array(vec![pool.address.parse().unwrap()]));
    
                        let mut stream = event.subscribe_with_meta().await.unwrap();
                        while let Some(Ok((log, meta))) = stream.next().await {
                            if let Some(mut pool_meta) = match pool.clone().provider {
                                LiquidityProviders::UniswapV3(pool_meta) => Some(pool_meta),
                                _ => None
                            } {
                                let mut updated_meta = get_complete_pool_data_batch_request(vec![H160::from_str(&pool_meta.address).unwrap()], client.clone())
                                    .await
                                    .unwrap()
                                    .first()
                                    .unwrap()
                                    .to_owned();
                                updated_meta.factory_address = pool_meta.factory_address;

                                pool.x_amount = updated_meta.token_a_amount;
                                pool.y_amount = updated_meta.token_b_amount;
                                pool.provider = LiquidityProviders::UniswapV3(updated_meta);
                                let mut w = pools.write().await;
                                w.insert(pool.address.clone(), pool.clone());
                            }
                            let event = PoolUpdateEvent {
                                pool: pool.clone(),
                                block_number: meta.block_number.as_u64(),
                                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis(),
                            };

                            let res = sub.send(Box::new(event.clone())).await.map_err(|e| info!("sync_service> UniswapV3 Send Error {:?}", e));
                        }
                    }));
                }
                futures::future::join_all(joins).await;
            });
        })
    }
}

impl EventEmitter<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>> for UniSwapV3 {
    fn get_subscribers(&self) -> Arc<std::sync::RwLock<Vec<AsyncSender<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>>>> {
        self.pending_subscribers.clone()
    }
    fn emit(&self) -> std::thread::JoinHandle<()> {
        let pools = self.pools.clone();
        let subscribers = self.pending_subscribers.clone();
        let node_url = self.nodes.next_free();

        std::thread::spawn(move || {
            let mut rt = Runtime::new().unwrap();
            let pools = pools.clone();

            rt.block_on(async move {
                let client = Arc::new(
                    Provider::<Ws>::connect(&node_url)
                        .await
                        .unwrap(),
                );
                let latest_block = client.get_block_number().await.unwrap();
                let pending_stream = client.watch_pending_transactions().await.unwrap();
                let mut stream = pending_stream.transactions_unordered(usize::MAX);
                let subscribers = subscribers.read().unwrap();
                let sub = Arc::new(subscribers.first().unwrap().clone());
                drop(subscribers);
                while let Some(tx_result) = stream.next().await {
                    match tx_result {
                        Err(e) => {
                            debug!("{:?}", e);
                        }
                        Ok(tx) => {
                            if tx.to != Some(H160::from_str(crate::uniswap_v2::UNISWAP_UNIVERSAL_ROUTER).unwrap()) {
                                continue;
                            }
                            let client = client.clone();
                            let sub = sub.clone();
                            let pools = pools.clone();
                            tokio::task::spawn(async move {
                                let pools = pools
                                .read()
                                .await
                                .values()
                                .cloned()
                                .collect::<Vec<Pool>>();
                                if pools.len() <= 0 {
                                    return ;
                                }


                                let now = U256::from(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());

                                if tx.to == Some(H160::from_str(crate::uniswap_v2::UNISWAP_UNIVERSAL_ROUTER).unwrap()) {
                                    if let Ok(decoded) = crate::abi::decode_universal_router_execute(&tx.input) {
                                        if decoded.deadline < now {
                                            return
                                        }
                                        for i in 0..decoded.commands.len() {
                                            let command = decoded.commands[i] & 0x3f;
                                            if command == 0x00 {
                                                let decoded: crate::uniswap_v2::V2ExactInInput = crate::uniswap_v2::V2ExactInInput::decode(&decoded.inputs[i]).unwrap();

                                                if decoded.path.len() > 2 {
                                                    return
                                                }
                                                if let Some(pool) = pools.iter().find(|p|
                                                    (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) ||
                                                        (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex()))) {
                                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                        (pool.x_amount, pool.y_amount)
                                                    } else {
                                                        (pool.y_amount, pool.x_amount)
                                                    };

                                                    let mut meta = match &pool.provider {
                                                        LiquidityProviders::UniswapV3(meta) => meta.clone(),
                                                        _ => return
                                                    };
                                                    let amount_out = match meta.simulate_swap_mut(pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()), decoded.amount_in, true) {
                                                        Ok(amount) => amount,
                                                        Err(e) => {
                                                            debug!("{:?}", e);
                                                            return
                                                        }
                                                    };

                                                    if amount_out < decoded.amount_out_min {
                                                        trace!("Output less than minimum");
                                                        return
                                                    }

                                                    meta.tick_bitmap_x_y = match get_uniswap_v3_tick_data_batch_request(&meta, meta.tick, true, 100, None, client.clone()).await {
                                                        Ok(res) => res,
                                                        _ => return
                                                    };

                                                    meta.tick_bitmap_y_x = match get_uniswap_v3_tick_data_batch_request(&meta, meta.tick, false, 100, None, client.clone()).await {
                                                        Ok(res) => res,
                                                        _ => return
                                                    };
                                                    let mut mutated_pool = pool.clone();
                                                    mutated_pool.provider = LiquidityProviders::UniswapV3(meta);
                                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                        (mutated_pool.x_amount.saturating_add(decoded.amount_in), mutated_pool.y_amount.saturating_sub(amount_out))
                                                    } else {
                                                        (mutated_pool.x_amount.saturating_sub(amount_out), mutated_pool.y_amount.saturating_add(decoded.amount_in))
                                                    };
                                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                                    info!("V3ExactInput: {} {:?} {} {:?}", decoded.amount_in, decoded, pool, amount_out);
                                                    let event = PendingPoolUpdateEvent {
                                                        pool: mutated_pool,
                                                        pending_tx: tx.clone(),
                                                        timestamp: 0,
                                                    };
                                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV3 Send Error {:?}", e)).unwrap();
                                                } else {
                                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));                                                }
                                            } else if command == 0x09 {
                                                let decoded: crate::uniswap_v2::V2ExactOutInput = crate::uniswap_v2::V2ExactOutInput::decode(&decoded.inputs[i]).unwrap();

                                                if decoded.path.len() > 2 {
                                                    return
                                                }

                                                if let Some(pool) = pools.iter().find(|p| (p.x_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.y_address == hex_to_address_string(decoded.path[1].encode_hex())) || (p.y_address == hex_to_address_string(decoded.path[0].encode_hex()) && p.x_address == hex_to_address_string(decoded.path[1].encode_hex()))) {
                                                    let (source_amount, dest_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                        (pool.x_amount, pool.y_amount)
                                                    } else {
                                                        (pool.y_amount, pool.x_amount)
                                                    };
                                                    let mut meta = match &pool.provider {
                                                        LiquidityProviders::UniswapV3(meta) => meta.clone(),
                                                        _ => return
                                                    };
                                                    let amount_in = match meta.simulate_swap_mut(pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()), decoded.amount_out, false) {
                                                        Ok(amount) => amount,
                                                        Err(e) => {
                                                            debug!("{:?}", e);
                                                            return
                                                        }
                                                    };

                                                    if amount_in > decoded.amount_in_max {
                                                        trace!("Insufficient amount for swap");
                                                        return
                                                    }

                                                    meta.tick_bitmap_x_y = match get_uniswap_v3_tick_data_batch_request(&meta, meta.tick, true, 100, None, client.clone()).await {
                                                        Ok(res) => res,
                                                        _ => return
                                                    };

                                                    meta.tick_bitmap_y_x = match get_uniswap_v3_tick_data_batch_request(&meta, meta.tick, false, 100, None, client.clone()).await {
                                                        Ok(res) => res,
                                                        _ => return
                                                    };
                                                    let mut mutated_pool = pool.clone();
                                                    mutated_pool.provider = LiquidityProviders::UniswapV3(meta);
                                                    (mutated_pool.x_amount, mutated_pool.y_amount) = if pool.x_address == hex_to_address_string(decoded.path[0].encode_hex()) {
                                                        (mutated_pool.x_amount.saturating_add(amount_in), mutated_pool.y_amount.saturating_sub(decoded.amount_out))
                                                    } else {
                                                        (mutated_pool.x_amount.saturating_sub(decoded.amount_out), mutated_pool.y_amount.saturating_add(amount_in))
                                                    };
                                                    debug!("Pre balance X: {} Y: {}\nPost balance X: {} Y: {}", pool.x_amount, pool.y_amount, mutated_pool.x_amount, mutated_pool.y_amount);
                                                    let event = PendingPoolUpdateEvent {
                                                        pool: mutated_pool,
                                                        pending_tx: tx.clone(),
                                                        timestamp: 0,
                                                    };
                                                    let res = sub.send(Box::new(event.clone())).await.map_err(|e| error!("sync_service> UniswapV3 Send Error {:?}", e)).unwrap();

                                                    info!("V3ExactOut: {:?}", decoded)
                                                } else {
                                                    trace!("Pair {} {} not being tracked", hex_to_address_string(decoded.path[0].encode_hex()), hex_to_address_string(decoded.path[1].encode_hex()));
                                                }
                                            }
                                        }
                                    } else {
                                        trace!("Couldn't decode universal router input {:?} {}", crate::abi::decode_universal_router_execute(&tx.input), &tx.input);
                                    }
                                } else {
                                    trace!("Transaction is not to uniswap v3, skipping...");
                                    return;
                                }
                            });
                        }
                    }
                }


            })
        })
    }
}

