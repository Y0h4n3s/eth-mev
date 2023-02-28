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
use serde::{Deserialize, Serialize};
use crate::{CHECKED_COIN, U256};
use crate::uniswap_v3::UniswapV3Metadata;
abigen!(
    GetUniswapV3PoolDataBatchRequest,
    "src/abi/uniswap_v3/GetUniswapV3PoolDataBatchRequest.json";
    GetUniswapV3TickData,
    "src/abi/uniswap_v3/GetUniswapV3TickData.json";
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


#[derive(Serialize, Deserialize,Decode, Encode, Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Default)]
pub struct UniswapV3TickData {
    pub initialized: bool,
    pub tick: i32,
    pub liquidity_net: i128,
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
            )
                .as_i128();

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
