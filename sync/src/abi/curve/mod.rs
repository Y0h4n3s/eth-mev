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
use tracing::{error, info};
use crate::balancer_weighted::BalancerWeigtedMetadata;
use hex::FromHex;
use itertools::Itertools;
use nom::HexDisplay;
use crate::uniswap_v2::UniswapV2Metadata;
use crate::abi::uniswap_v2::UniswapV2DataAggregator;
use crate::curve::CurvePlainMetadata;
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
) -> Result<Vec<CurvePlainMetadata>> {
    let mut target_addresses = vec![];
    for pair in pairs.iter() {
        target_addresses.push(Token::Address(pair.clone()));
    }


    let mut final_pairs = vec![];
    let constructor_args = Token::Tuple(vec![Token::Array(target_addresses)]);

    let deployer =
        CurvePlainDataAggregator::deploy(middleware.clone(), constructor_args).unwrap();

    loop {
        let call = deployer.call_raw().await;
        if let Ok(return_data) = call {
            let return_data_tokens = ethers::abi::decode(
                &[ParamType::Array(Box::new(ParamType::Tuple(vec![
                    ParamType::Address,   // address
                    ParamType::Array(Box::new(ParamType::Address)),  // tokens
                    ParamType::Array(Box::new(ParamType::Uint(256))),  // balances
                    ParamType::Array(Box::new(ParamType::Uint(256))),  // decimals
                    ParamType::Uint(256),  // amp
                    ParamType::Uint(256),  // fee
                    ParamType::Uint(256),  // block number

                ])))],
                &return_data,
            )?;


            for tokens in return_data_tokens {
                if let Some(tokens_arr) = tokens.into_array() {
                    for tup in tokens_arr {
                        if let Some(pool_data) = tup.into_tuple() {
                            if !pool_data[0].to_owned().into_address().unwrap().is_zero() {
                                //Update the pool data
                                let u_pair = CurvePlainMetadata {
                                    address: hex_to_address_string(
                                        pool_data[0]
                                            .to_owned()
                                            .into_address()
                                            .unwrap()
                                            .encode_hex(),
                                    ),
                                    tokens:
                                        pool_data[1]
                                            .to_owned()
                                            .into_array()
                                            .unwrap()
                                            .into_iter()
                                            .filter(|t| !t.clone().into_address().unwrap().is_zero())
                                            .map(|addr|  hex_to_address_string(addr.into_address().unwrap().encode_hex()))
                                            .collect_vec(),
                                    balances:
                                        pool_data[2]
                                            .to_owned()
                                            .into_array()
                                            .unwrap()
                                            .into_iter()
                                            .filter(|t| !t.clone().into_uint().unwrap().is_zero())
                                            .map(|addr|  addr.into_uint().unwrap().into())
                                            .collect::<Vec<U256>>(),
                                    decimals:
                                        pool_data[3]
                                            .to_owned()
                                            .into_array()
                                            .unwrap()
                                            .into_iter()
                                            .filter(|t| !t.clone().into_uint().unwrap().is_zero())
                                            .map(|addr|  addr.into_uint().unwrap().as_u32() as u8).collect_vec(),
                                    fee: pool_data[4]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .into(),
                                    amp: pool_data[5]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .into(),
                                    block_number: pool_data[6]
                                        .to_owned()
                                        .into_uint()
                                        .unwrap()
                                        .as_u64(),
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
                    error!("{:?}", err);
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
