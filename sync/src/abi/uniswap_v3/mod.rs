use std::sync::Arc;

use crate::types::{UniSwapV3Pool, UniSwapV3Token};
use anyhow::{Error, Result};
use ethers::abi::AbiEncode;
use ethers::types::{Address, H160};
use ethers::{
    abi::{ParamType, Token},
    prelude::abigen,
    providers::Middleware,
    types::{Bytes, I256},
};
use ethers_providers::ProviderError::JsonRpcClientError;
use byte_slice_cast::AsByteSlice;
use ethers::prelude::builders::ContractCall;
use tokio::task::JoinHandle;
use std::str::FromStr;
use bincode::{Decode, Encode};
use nom::character::complete::i128;
use serde::{Deserialize, Serialize};
use tracing::info;
use crate::{CHECKED_COIN, U256};
use crate::uniswap_v3::UniswapV3Metadata;
abigen!(
    GetUniswapV3PoolDataBatchRequest,
    "src/abi/uniswap_v3/GetUniswapV3PoolDataBatchRequest.json";
    GetUniswapV3TickData,
    "src/abi/uniswap_v3/GetUniswapV3TickData.json";
    UniswapV3DataAggregator,
    "src/abi/uniswap_v3/UniswapV3DataAggregator.json";
);
abigen!(UniswapV3Pool, "src/abi/IUniswapV3Pool.json");
abigen!(IERC20, "src/abi/IERC20.json");


pub async fn get_pool_data_batch_request(
    pools: Vec<H160>,
    middleware: Arc<ethers_providers::Provider<ethers_providers::Ws>>,
) -> Result<Vec<UniSwapV3Pool>> {
    let blacklist = ["0x8de5977111c68c3fe95e63e7f7319dd5a01f77a0".to_string()];
    let mut target_addresses = vec![];
    for pool in pools.iter() {
        target_addresses.push(Token::Address(pool.clone()));
    }
    
    let mut handles: Vec<JoinHandle<anyhow::Result<UniSwapV3Pool>>> = vec![];
    for pool in pools {
        let provider = middleware.clone();
        handles.push(tokio::spawn(async move {
            let contract = UniswapV3Pool::new(pool.clone(), provider.clone());
            let token0_call = Box::pin(contract.token_0());
            let token1_call = Box::pin(contract.token_1());
            let liquidity_call = Box::pin(contract.liquidity());
            let fee_call = Box::pin(contract.fee());
            let tick_spacing_call = Box::pin(contract.fee());
            let slot_0_call = Box::pin(contract.slot_0());
            let (token0r, token1r, liquidityr, feer, tick_spacing_r, sqrt_pricer) =
                  (
                      token0_call.call().await.map_err(|e| Error::msg(format!("Missing function token_0() for {}", hex_to_address_string(pool.encode_hex())))),
                      token1_call.call().await.map_err(|e| Error::msg(format!("Missing function token_1() for {}", hex_to_address_string(pool.encode_hex())))),
                      liquidity_call.call().await.map_err(|e| Error::msg(format!("Missing function liquidity() for {}", hex_to_address_string(pool.encode_hex())))),
                      fee_call.call().await.map_err(|e| Error::msg(format!("Missing function fee() for {}", hex_to_address_string(pool.encode_hex())))),
                      tick_spacing_call.call().await.map_err(|e| Error::msg(format!("Missing function fee() for {}", hex_to_address_string(pool.encode_hex())))),
                      slot_0_call.call().await.map_err(|e| Error::msg(format!("Missing function slot_0() for {}", hex_to_address_string(pool.encode_hex()))))
                  );

            if token0r.is_err() || token1r.is_err() || liquidityr.is_err() || feer.is_err() || tick_spacing_r.is_err() || sqrt_pricer.is_err() {
                return Err(Error::msg("Faild to load"))
            }
            let s = sqrt_pricer.unwrap();
            let (token0, token1, liquidity, fee, tick_spacing, sqrt_price, tick): (H160, H160, u128, u32, u32, U256, i32) = (token0r.unwrap(), token1r.unwrap(), liquidityr.unwrap(), feer.unwrap(), tick_spacing_r.unwrap(), s.0, s.1);
            let token0_contract = IERC20::new(token0, provider.clone());
            let token1_contract = IERC20::new(token1, provider.clone());
            let token_0_decimals_call = Box::pin(token0_contract.decimals());
            let token_1_decimals_call = Box::pin(token1_contract.decimals());
            let token_0_balance_call = Box::pin(token0_contract.balance_of(pool.clone()));
            let token_1_balance_call = Box::pin(token1_contract.balance_of(pool.clone()));
            let (token_0_decimalsr, token_1_decimalsr, token_0_balancer, token_1_balancer)  = (
                  token_0_decimals_call.call().await.map_err(|e|Error::msg(format!("No decimals for {}", hex_to_address_string(token0.encode_hex())))),
                  token_1_decimals_call.call().await.map_err(|e|Error::msg(format!("No decimals for {}", hex_to_address_string(token1.encode_hex())))),
                  token_0_balance_call.call().await.map_err(|e|Error::msg(format!("No balance for {}", hex_to_address_string(token0.encode_hex())))),
                  token_1_balance_call.call().await.map_err(|e|Error::msg(format!("No balance for {}", hex_to_address_string(token1.encode_hex())))),
                  );

            if token_0_decimalsr.is_err() || token_1_decimalsr.is_err() || token_0_balancer.is_err() || token_1_balancer.is_err() {
                return Err(Error::msg("Faild to load"))

            }
            let (token_0_decimals, token_1_decimals, token_0_balance, token_1_balance) : (u8, u8, U256, U256) = (token_0_decimalsr.unwrap(), token_1_decimalsr.unwrap(), token_0_balancer.unwrap(), token_1_balancer.unwrap());
            if hex_to_address_string(token0.encode_hex()) == CHECKED_COIN.clone() {
                if token_0_balance.le(&U256::from(10_u128.pow(18))) {
                    return Err(Error::msg(format!("Low Liquidity for pool {}", hex_to_address_string(pool.encode_hex()))))
                }
            }


            if hex_to_address_string(token1.encode_hex()) == CHECKED_COIN.clone() {
                if token_1_balance.le(&U256::from(10_u128.pow(18))) {
                    return Err(Error::msg(format!("Low Liquidity for pool {}", hex_to_address_string(pool.encode_hex()))))
                }
            }
            Ok(UniSwapV3Pool {
                id: hex_to_address_string(pool.encode_hex()),
                fee,
                tick_spacing: tick_spacing as i32,
                tick: tick,
                token0: UniSwapV3Token {
                    id: hex_to_address_string(token0.encode_hex()),
                    ..Default::default()
                },
                token1: UniSwapV3Token {
                    id: hex_to_address_string(token1.encode_hex()),
                    ..Default::default()
                },
                liquidity,
                sqrt_price: sqrt_price.to_string(),
                token0_decimals: token_0_decimals,
                token1_decimals: token_1_decimals,
                ..Default::default()
            })
        }));
    }
    return Ok(futures::future::join_all(handles).await.into_iter().filter_map(|p| {
        if p.is_err() {
            None
        } else {
            if p.as_ref().unwrap().is_err() {
                None
            } else {
                Some(p.unwrap().as_ref().unwrap().clone())

            }
        }
    }).collect());

}
const MAX_TICK_SIZE: u64 = 887272;
pub async fn get_complete_pool_data_batch_request<M: Middleware>(
    pairs: Vec<H160>,
    middleware: &Arc<M>,
) -> Result<Vec<UniswapV3Metadata>> {
    let mut target_addresses = vec![];
    for pair in pairs.iter() {
        target_addresses.push(Token::Address(pair.clone()));
    }


    let mut final_pairs = vec![];
    let constructor_args = Token::Tuple(vec![Token::Array(target_addresses)]);

    let deployer =
        UniswapV3DataAggregator::deploy(middleware.clone(), constructor_args).unwrap();

    loop {
        let call = deployer.call_raw().await;
        if let Ok(return_data) = call {
            let return_data_tokens = ethers::abi::decode(
                &[ParamType::Array(Box::new(ParamType::Tuple(vec![
                    ParamType::Address,   // address
                    ParamType::Address,   // token a
                    ParamType::Uint(8),   // token a decimals
                    ParamType::Address,   // token b
                    ParamType::Uint(8),   // token b decimals
                    ParamType::Uint(128), // liquidity
                    ParamType::Uint(160), // sqrtPrice
                    ParamType::Int(24),   // tick
                    ParamType::Int(24),   // tickSpacing
                    ParamType::Uint(24),  // fee
                    ParamType::Int(128),  // liquidityNet
                    ParamType::Uint(256),  // token a amount
                    ParamType::Uint(256),  // token b amount
                    ParamType::Uint(256),  // block number
                    ParamType::Array(Box::new(ParamType::Tuple(vec![
                        ParamType::Bool,
                        ParamType::Int(24),
                        ParamType::Int(128)
                    ]))),                  // tickBitmapXY
                    ParamType::Array(Box::new(ParamType::Tuple(vec![
                        ParamType::Bool,
                        ParamType::Int(24),
                        ParamType::Int(128)
                    ]))),                    // tickBitmapXY
                ])))],
                &return_data,
            )?;

            let mut pool_idx = 0;

            for tokens in return_data_tokens {
                if let Some(tokens_arr) = tokens.into_array() {
                    for tup in tokens_arr {
                        if let Some(pool_data) = tup.into_tuple() {
                            if !pool_data[1].to_owned().into_address().unwrap().is_zero()
                                && !pool_data[3].to_owned().into_address().unwrap().is_zero()
                            {
                                //Update the pool data

                                    let u_pair = UniswapV3Metadata {
                                        address: hex_to_address_string(
                                            pool_data[0]
                                                .to_owned()
                                                .into_address()
                                                .unwrap()
                                                .encode_hex(),
                                        ),
                                        token_a: hex_to_address_string(
                                                pool_data[1]
                                                    .to_owned()
                                                    .into_address()
                                                    .unwrap()
                                                    .encode_hex(),
                                            ),
                                        token_b: hex_to_address_string(
                                                pool_data[3]
                                                    .to_owned()
                                                    .into_address()
                                                    .unwrap()
                                                    .encode_hex(),
                                            ),
                                        token_a_decimals: pool_data[2]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .as_u32() as u8,
                                        token_b_decimals: pool_data[4]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .as_u32() as u8,
                                        liquidity: pool_data[5]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .into(),
                                        sqrt_price: pool_data[6]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .into(),
                                        tick: I256::from_raw(pool_data[7]
                                            .to_owned()
                                            .into_int()
                                            .unwrap())
                                            .as_i32(),
                                        tick_spacing: pool_data[8]
                                            .to_owned()
                                            .into_int()
                                            .unwrap()
                                            .to_string()
                                            .parse::<i32>()
                                            .unwrap(),
                                        fee: pool_data[9]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .as_u32(),
                                        liquidity_net: I256::from_raw(pool_data[10]
                                            .to_owned()
                                            .into_int()
                                            .unwrap()),
                                        token_a_amount: pool_data[11]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .into(),
                                        token_b_amount: pool_data[12]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .into(),
                                        block_number: pool_data[13]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .as_u64(),
                                        tick_bitmap_x_y: pool_data[14]
                                            .to_owned()
                                            .into_array()
                                            .unwrap()
                                            .into_iter()
                                            .map(|t| UniswapV3TickData::from_tokens(t.into_tuple().unwrap()))
                                            .collect::<Vec<UniswapV3TickData>>(),
                                        tick_bitmap_y_x: pool_data[15]
                                            .to_owned()
                                            .into_array()
                                            .unwrap()
                                            .into_iter()
                                            .map(|t| UniswapV3TickData::from_tokens(t.into_tuple().unwrap()))
                                            .collect::<Vec<UniswapV3TickData>>(),

                                        ..Default::default()
                                    };
                                    final_pairs.push(u_pair);
                                }
                        }
                    }
                }
            }
            break;
        } else {
            match call.unwrap_err() {
                JsonRpcClientError(err) => {
                    // eprintln!("{:?}", err);
                    continue;
                }
                _ => break,
            }
        }
    }

    Ok(final_pairs)
}


#[derive(Serialize, Deserialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct UniswapV3TickData {
    pub initialized: bool,
    pub tick: i32,
    pub liquidity_net: I256,
}

impl UniswapV3TickData {
    fn from_tokens(tokens: Vec<Token>) -> Self {

        Self {
            initialized: tokens[0]
                .to_owned()
                .into_bool()
                .unwrap(),
            tick: I256::from_raw(tokens[1]
                .to_owned()
                .into_int()
                .unwrap()
            ).as_i32(),
            liquidity_net: I256::from_raw(tokens[2]
                .to_owned()
                .into_int()
                .unwrap()),

        }
    }
}

pub async fn get_uniswap_v3_tick_data_batch_request(
    pool: &UniswapV3Metadata,
    tick_start: i32,
    zero_for_one: bool,
    num_ticks: u16,
    block_number: Option<u64>,
    middleware: Arc<ethers_providers::Provider<ethers_providers::Ws>>,
) -> Result<Vec<UniswapV3TickData>> {
    let constructor_args = Token::Tuple(vec![
        Token::Address(Address::from_str(&pool.address).unwrap()),
        Token::Bool(zero_for_one),
        Token::Int(I256::from(tick_start).into_raw()),
        Token::Uint(U256::from(num_ticks)),
        Token::Int(I256::from(pool.tick_spacing).into_raw()),
    ]);

    let deployer =
        GetUniswapV3TickData::deploy(middleware.clone(), constructor_args).unwrap();

    let return_data: Bytes = if block_number.is_some() {
        deployer.block(block_number.unwrap()).call_raw().await?
    } else {
        deployer.call_raw().await?
    };

    let return_data_tokens = ethers::abi::decode(
        &[
            ParamType::Array(Box::new(ParamType::Tuple(vec![
                ParamType::Bool,
                ParamType::Int(24),
                ParamType::Int(128),
            ]))),
            ParamType::Uint(32),
        ],
        &return_data,
    )?;

    //TODO: handle these errors instead of using expect
    let tick_data_array = return_data_tokens[0]
        .to_owned()
        .into_array()
        .expect("Failed to convert initialized_ticks from Vec<Token> to Vec<i128>");

    let mut tick_data = vec![];

    for tokens in tick_data_array {
        if let Some(tick_data_tuple) = tokens.into_tuple() {
            let initialized = tick_data_tuple[0]
                .to_owned()
                .into_bool()
                .expect("Could not convert token to bool");

            let initialized_tick = I256::from_raw(
                tick_data_tuple[1]
                    .to_owned()
                    .into_int()
                    .expect("Could not convert token to int"),
            )
                .as_i32();

            let liquidity_net = I256::from_raw(
                tick_data_tuple[2]
                    .to_owned()
                    .into_int()
                    .expect("Could not convert token to int"),
            );

            tick_data.push(UniswapV3TickData {
                initialized,
                tick: initialized_tick,
                liquidity_net,
            });
        }
    }

    let block_number = return_data_tokens[1]
        .to_owned()
        .into_uint()
        .expect("Failed to convert block_number from Token to U64");

    Ok(tick_data)
}

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}
