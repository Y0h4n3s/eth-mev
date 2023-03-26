use crate::types::{UniSwapV2Pair, UniSwapV2Token};
use anyhow::Result;
use ethers::abi::{AbiEncode, FixedBytes};
use ethers::{
    abi::{ParamType, Token},
    prelude::abigen,
    providers::Middleware,
    types::{Bytes, H160, U256},
};
use ethers_providers::ProviderError::JsonRpcClientError;
use std::sync::Arc;
use tracing::info;
use crate::balancer_weighted::BalancerWeigtedMetadata;
use hex::FromHex;
use nom::HexDisplay;
use crate::uniswap_v2::UniswapV2Metadata;
use crate::abi::uniswap_v2::UniswapV2DataAggregator;
abigen!(
    CurvePlainDataAggregator,
    "src/abi/curve/CurvePlainDataAggregator.json"
);
abigen!(CurvePlainPairsBatch,
    "src/abi/curve/CurvePlainPairsBatch.json");


pub async fn get_pairs_batch_request<M: Middleware>(
    factory: H160,
    from: U256,
    middleware: Arc<M>,
) -> Result<Vec<H160>> {
    let mut pairs = vec![];

    let constructor_args = Token::Tuple(vec![
        Token::Address(factory),
        Token::Uint(from)
    ]);

    let deployer = CurvePlainPairsBatch::deploy(middleware, constructor_args).unwrap();
    let return_data: Bytes = deployer.call_raw().await?;

    let return_data_tokens = ethers::abi::decode(
        &[ParamType::Array(Box::new(ParamType::Address))],
        &return_data,
    )?;

    for token_array in return_data_tokens {
        if let Some(arr) = token_array.into_array() {
            for token in arr {
                if let Some(addr) = token.into_address() {
                    if !addr.is_zero() {
                        pairs.push(addr);
                    }
                }
            }
        }
    }

    Ok(pairs)
}




pub async fn get_complete_pool_data_batch_request<M: Middleware>(
    pairs: Vec<H160>,
    middleware: &Arc<M>,
) -> Result<Vec<UniswapV2Metadata>> {
    let mut target_addresses = vec![];
    for pair in pairs.iter() {
        target_addresses.push(Token::Address(pair.clone()));
    }


    let mut final_pairs = vec![];
    let constructor_args = Token::Tuple(vec![Token::Array(target_addresses)]);

    let deployer =
        UniswapV2DataAggregator::deploy(middleware.clone(), constructor_args).unwrap();

    loop {
        let call = deployer.call_raw().await;
        if let Ok(return_data) = call {
            let return_data_tokens = ethers::abi::decode(
                &[ParamType::Array(Box::new(ParamType::Tuple(vec![
                    ParamType::Address,   // address
                    ParamType::Uint(256),  // token a amount
                    ParamType::Uint(256),  // token b amount
                    ParamType::Uint(256),  // token a balance
                    ParamType::Uint(256),  // token b balance
                    ParamType::Uint(256),  // block number
                    ParamType::Address,   // token a
                    ParamType::Uint(8),   // token a decimals
                    ParamType::Address,   // token b
                    ParamType::Uint(8),   // token b decimals

                ])))],
                &return_data,
            )?;


            for tokens in return_data_tokens {
                if let Some(tokens_arr) = tokens.into_array() {
                    for tup in tokens_arr {
                        if let Some(pool_data) = tup.into_tuple() {
                            if !pool_data[0].to_owned().into_address().unwrap().is_zero() {
                                //Update the pool data
                                let u_pair = UniswapV2Metadata {
                                    address: hex_to_address_string(
                                        pool_data[0]
                                            .to_owned()
                                            .into_address()
                                            .unwrap()
                                            .encode_hex(),
                                    ),
                                    reserve0: pool_data[1]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .into(),
                                    reserve1: pool_data[2]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .into(),
                                    balance0: pool_data[3]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .into(),
                                    balance1: pool_data[4]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .into(),
                                    block_number: pool_data[5]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .as_u64(),
                                    token0: hex_to_address_string(
                                        pool_data[6]
                                            .to_owned()
                                            .into_address()
                                            .unwrap()
                                            .encode_hex(),
                                    ),
                                    token0_decimals: pool_data[7]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .as_u32()
                                        as u8,
                                    token1: hex_to_address_string(
                                        pool_data[8]
                                            .to_owned()
                                            .into_address()
                                            .unwrap()
                                            .encode_hex(),
                                    ),
                                    token1_decimals: pool_data[9]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .as_u32()
                                        as u8,
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

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}
