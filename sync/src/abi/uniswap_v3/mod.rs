use std::sync::Arc;

use ethers::{
    abi::{ParamType, Token},
    prelude::abigen,
    providers::Middleware,
    types::{Bytes, I256},
};
use ethers::types::H160;
use anyhow::Result;
use ethers_providers::ProviderError::JsonRpcClientError;
use crate::types::{UniSwapV3Pool, UniSwapV3Token};
use ethers::abi::AbiEncode;
abigen!(
    GetUniswapV3PoolDataBatchRequest,
    "src/abi/uniswap_v3/GetUniswapV3PoolDataBatchRequest.json";
    SyncUniswapV3PoolBatchRequest,
    "src/abi/uniswap_v3/SyncUniswapV3PoolBatchRequest.json";
);

pub async fn get_pool_data_batch_request<M: Middleware>(
    pools: Vec<H160>,
    middleware: Arc<M>,
) -> Result<Vec<UniSwapV3Pool>> {
    let mut target_addresses = vec![];
    
    for pool in pools.iter() {
        target_addresses.push(Token::Address(pool.clone()));
    }
    
    let constructor_args = Token::Tuple(vec![Token::Array(target_addresses)]);
    let deployer =
          GetUniswapV3PoolDataBatchRequest::deploy(middleware.clone(), constructor_args).unwrap();
    let mut final_pools = vec![];
    
    loop {
        let call = deployer.call_raw().await;
        if let Ok(return_data) =  call {
    let return_data_tokens = ethers::abi::decode(
        &[ParamType::Array(Box::new(ParamType::Tuple(vec![
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
        ])))],
        &return_data,
    )?;
    
    let mut pool_idx = 0;
    
    //Update pool data
    for tokens in return_data_tokens {
        if let Some(tokens_arr) = tokens.into_array() {
            for tup in tokens_arr {
                if let Some(pool_data) = tup.into_tuple() {
                    //If the pool token A is not zero, signaling that the pool data was populated
                    if !pool_data[0].to_owned().into_address().unwrap().is_zero() {
                        //Update the pool data
                        if let pool_address = pools.get(pool_idx).unwrap() {
                            let pool = UniSwapV3Pool {
                                id: hex_to_address_string(pool_address.encode_hex()),
                                feeTier: pool_data[8].to_owned().into_uint().unwrap().as_u64().to_string(),
                                token0: UniSwapV3Token {
                                    id: hex_to_address_string(pool_data[0].to_owned().into_address().unwrap().encode_hex()),
                                    ..Default::default()
                                },
                                token1: UniSwapV3Token {
                                    id: hex_to_address_string(pool_data[2].to_owned().into_address().unwrap().encode_hex()),
                                    ..Default::default()
                                },
                                volumeUSD: "".to_string(),
                                totalValueLockedToken0: "".to_string(),
                                totalValueLockedToken1: "".to_string(),
                                totalValueLockedUSD: "".to_string(),
                                __typename: "".to_string()
                            };
                            final_pools.push(pool);
                        }
                        pool_idx += 1;
                    }
                }
            }
        }
    }
            break
    
        } else {
            match call.unwrap_err() {
                JsonRpcClientError(err) => {
                    // eprintln!("{:?}", err);
                    continue
                }
                _ => break
            }
        }
    
    }
    
    Ok(final_pools)
    
}

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}
