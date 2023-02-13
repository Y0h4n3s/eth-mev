use std::sync::Arc;

use crate::types::{UniSwapV3Pool, UniSwapV3Token};
use anyhow::{Error, Result};
use ethers::abi::AbiEncode;
use ethers::types::H160;
use ethers::{
    abi::{ParamType, Token},
    prelude::abigen,
    providers::Middleware,
    types::{Bytes, I256},
};
use ethers_providers::ProviderError::JsonRpcClientError;
use byte_slice_cast::AsByteSlice;
use ethers::prelude::builders::ContractCall;

use crate::{CHECKED_COIN, U256};
abigen!(
    GetUniswapV3PoolDataBatchRequest,
    "src/abi/uniswap_v3/GetUniswapV3PoolDataBatchRequest.json";
    SyncUniswapV3PoolBatchRequest,
    "src/abi/uniswap_v3/SyncUniswapV3PoolBatchRequest.json";
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
    
    let mut handles = vec![];
    for pool in pools {
        let provider = middleware.clone();
        handles.push(tokio::spawn(async move {
            let contract = UniswapV3Pool::new(pool.clone(), provider.clone());
            let token0_call = Box::pin(contract.token_0());
            let token1_call = Box::pin(contract.token_1());
            let liquidity_call = Box::pin(contract.liquidity());
            let fee_call = Box::pin(contract.fee());
            let slot_0_call = Box::pin(contract.slot_0());
            let (token0, token1, liquidity, fee, sqrt_price): (H160, H160, u128, u32, U256) =
                  (
                      token0_call.call().await.map_err(|e| Error::msg(format!("Missing function token_0() for {}", hex_to_address_string(pool.encode_hex())))).unwrap(),
                      token1_call.call().await.map_err(|e| Error::msg(format!("Missing function token_1() for {}", hex_to_address_string(pool.encode_hex())))).unwrap(),
                      liquidity_call.call().await.map_err(|e| Error::msg(format!("Missing function liquidity() for {}", hex_to_address_string(pool.encode_hex())))).unwrap(),
                      fee_call.call().await.map_err(|e| Error::msg(format!("Missing function fee() for {}", hex_to_address_string(pool.encode_hex())))).unwrap(),
                      slot_0_call.call().await.map_err(|e| Error::msg(format!("Missing function slot_0() for {}", hex_to_address_string(pool.encode_hex())))).unwrap().0
                  );
            
            let token0_contract = IERC20::new(token0, provider.clone());
            let token1_contract = IERC20::new(token1, provider.clone());
            let token_0_decimals_call = Box::pin(token0_contract.decimals());
            let token_1_decimals_call = Box::pin(token1_contract.decimals());
            let token_0_balance_call = Box::pin(token0_contract.balance_of(pool.clone()));
            let token_1_balance_call = Box::pin(token1_contract.balance_of(pool.clone()));
            let (token_0_decimals, token_1_decimals, token_0_balance, token_1_balance) : (u8, u8, U256, U256) = (
                  token_0_decimals_call.call().await.map_err(|e|Error::msg(format!("No decimals for {}", hex_to_address_string(token0.encode_hex())))).unwrap(),
                  token_1_decimals_call.call().await.map_err(|e|Error::msg(format!("No decimals for {}", hex_to_address_string(token1.encode_hex())))).unwrap(),
                  token_0_balance_call.call().await.map_err(|e|Error::msg(format!("No balance for {}", hex_to_address_string(token0.encode_hex())))).unwrap(),
                  token_1_balance_call.call().await.map_err(|e|Error::msg(format!("No balance for {}", hex_to_address_string(token1.encode_hex())))).unwrap(),
                  );
            if hex_to_address_string(token0.encode_hex()) == CHECKED_COIN.clone() {
                if token_0_balance.le(&U256::from(10_u128.pow(18))) {
                    panic!("Low Liquidity for pool {}", hex_to_address_string(pool.encode_hex()))
                }
            }
            if hex_to_address_string(token1.encode_hex()) == CHECKED_COIN.clone() {
                if token_1_balance.le(&U256::from(10_u128.pow(18))) {
                    panic!("Low Liquidity for pool {}", hex_to_address_string(pool.encode_hex()))
                }
            }
            UniSwapV3Pool {
                id: hex_to_address_string(pool.encode_hex()),
                fee,
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
            }
        }));
    }
    return Ok(futures::future::join_all(handles).await.into_iter().filter_map(|p| {
        if p.is_err() {
            None
        } else {
            Some(p.unwrap())
        }
    }).collect());

}

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}
