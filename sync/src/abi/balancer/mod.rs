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

abigen!(
    BalancerWeightedDataAggregator,
    "src/abi/balancer/BalancerWeightedDataAggregator.json";
);

#[derive(Debug)]
pub struct BalancerWeightedPoolStatus {
    pub id: String,
    pub tokens: Vec<String>,
    pub balances: Vec<U256>
}

pub async fn get_complete_pool_data_batch_request<M: Middleware>(
    pairs: Vec<BalancerWeigtedMetadata>,
    middleware: Arc<M>,
) -> Result<Vec<BalancerWeightedPoolStatus>> {
    let mut target_addresses = vec![];
    for pair in pairs.iter() {
        target_addresses.push(Token::FixedBytes(FixedBytes::from_hex(pair.id.clone()[2..].to_string()).unwrap()));
    }


    let mut final_pairs = vec![];
    let constructor_args = Token::Tuple(vec![Token::Array(target_addresses)]);

    let deployer =
        BalancerWeightedDataAggregator::deploy(middleware.clone(), constructor_args).unwrap();

    loop {
        let call = deployer.call_raw().await;
        if let Ok(return_data) = call {
            let return_data_tokens = ethers::abi::decode(
                &[ParamType::Array(Box::new(ParamType::Tuple(vec![
                    ParamType::Uint(256),  // block number
                    ParamType::Array(Box::new(ParamType::Address)),
                    ParamType::Array(Box::new(ParamType::Uint(256)))

                ])))],
                &return_data,
            )?;

            for tokens in return_data_tokens {
                if let Some(tokens_arr) = tokens.into_array() {
                    for tup in tokens_arr {
                        if let Some(pool_data) = tup.into_tuple() {

                                let u_pair = BalancerWeightedPoolStatus {
                                    id:
                                        pool_data[0]
                                            .to_owned()
                                            .into_uint()
                                            .unwrap()
                                            .encode_hex(),
                                    tokens: pool_data[1]
                                        .to_owned()
                                        .into_array()
                                        .unwrap()
                                        .into_iter()
                                        .map(|t| hex_to_address_string(t.into_address().unwrap().encode_hex()))
                                        .collect::<Vec<String>>(),

                                    balances: pool_data[2]
                                        .to_owned()
                                        .into_array()
                                        .unwrap()
                                        .into_iter()
                                        .map(|t| t.into_uint().unwrap().into())
                                        .collect::<Vec<U256>>(),

                                };
                                final_pairs.push(u_pair);
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
