//! ABIs
//!
//! Contract ABIs are refactored into their own module to gracefully deal with allowing missing docs on the abigen macro.
#![allow(missing_docs)]
pub mod uniswap_v2;
pub mod uniswap_v3;
use ethers::{abi::AbiDecode, prelude::*};

abigen!(UniswapV3Pool, "src/abi/IUniswapV3Pool.json");
abigen!(UniswapV2Pair, "src/abi/IUniswapV2Pair.json");
abigen!(UniswapV2Router02, "src/abi/IUniswapV2Router02.json");
abigen!(UniswapV2Factory, "src/abi/IUniswapV2Factory.json");
abigen!(IERC20, "src/abi/IERC20.json");

