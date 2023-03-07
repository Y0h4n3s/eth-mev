//! ABIs
//!
//! Contract ABIs are refactored into their own module to gracefully deal with allowing missing docs on the abigen macro.
#![allow(missing_docs)]
pub mod uniswap_v2;
pub mod uniswap_v3;
use ethers::{abi::AbiDecode, prelude::{Bytes, abigen}};

abigen!(UniswapV2Pair, "src/abi/IUniswapV2Pair.json");
abigen!(UniswapV2Router02, "src/abi/IUniswapV2Router02.json");
abigen!(UniswapV2Factory, "src/abi/IUniswapV2Factory.json");
abigen!(UniversalRouter, "src/abi/IUniversalRouter.json");
abigen!(IERC20, "src/abi/IERC20.json");

// swapExactEthForTokens
pub fn decode_uniswap_router_swap_exact_eth_for_tokens(data: &Bytes) -> anyhow::Result<SwapExactETHForTokensCall> {
    SwapExactETHForTokensCall::decode(data).map_err(|e| anyhow::Error::from(e))
}
// swapExactTokensForTokens
pub fn decode_uniswap_router_swap_exact_tokens_for_tokens(data: &Bytes) -> anyhow::Result<SwapExactTokensForTokensCall> {
    SwapExactTokensForTokensCall::decode(data).map_err(|e| anyhow::Error::from(e))
}
// swapExactTokensForEth
pub fn decode_uniswap_router_swap_exact_tokens_for_eth(data: &Bytes) -> anyhow::Result<SwapExactTokensForETHCall> {
    SwapExactTokensForETHCall::decode(data).map_err(|e| anyhow::Error::from(e))
}
// swapTokensForExactTokens
pub fn decode_uniswap_router_swap_tokens_for_exact_tokens(data: &Bytes) -> anyhow::Result<SwapTokensForExactTokensCall> {
    SwapTokensForExactTokensCall::decode(data).map_err(|e| anyhow::Error::from(e))
}
// swapTokensForExactEth
pub fn decode_uniswap_router_swap_tokens_for_exact_eth(data: &Bytes) -> anyhow::Result<SwapTokensForExactETHCall> {
    SwapTokensForExactETHCall::decode(data).map_err(|e| anyhow::Error::from(e))
}
// swapEthForExactTokens
pub fn decode_uniswap_router_swap_eth_for_exact_tokens(data: &Bytes) -> anyhow::Result<SwapETHForExactTokensCall> {
    SwapETHForExactTokensCall::decode(data).map_err(|e| anyhow::Error::from(e))
}
pub fn decode_universal_router_execute(data: &Bytes) -> anyhow::Result<ExecuteCall> {
    ExecuteCall::decode(data).map_err(|e| anyhow::Error::from(e))
}
