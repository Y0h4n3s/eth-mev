#![allow(non_snake_case)]

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub struct UniSwapV2Token {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub decimals: String,
    pub totalLiquidity: String,
    pub derivedETH: String,
    pub __typename: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub struct UniSwapV3Token {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub decimals: String,
    pub derivedETH: String,
    pub __typename: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UniSwapV2Pair {
    pub id: String,
    pub token0: UniSwapV2Token,
    pub token1: UniSwapV2Token,
    pub reserve0: String,
    pub reserve1: String,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct UniSwapV3Pool {
    pub id: String,
    pub feeTier: String,
    pub token0: UniSwapV3Token,
    pub token1: UniSwapV3Token,
    pub volumeUSD: String,
    pub totalValueLockedToken0: String,
    pub totalValueLockedToken1: String,
    pub totalValueLockedUSD: String,
    pub __typename: String,
}
