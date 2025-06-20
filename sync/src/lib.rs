#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

#[macro_use]
extern crate serde_derive;
extern crate rmp_serde as rmps;
mod abi;
pub mod balancer_weighted;
mod events;
pub mod node_dispatcher;
pub mod sushiswap;
pub mod types;
pub mod uniswap_v2;
pub mod uniswap_v3;
pub mod curve;
use crate::balancer_weighted::BalancerWeigtedMetadata;
use crate::events::EventEmitter;
use crate::uniswap_v2::UniswapV2Metadata;
use crate::uniswap_v3::UniswapV3Metadata;
use anyhow::Error;
use async_std::sync::Arc;
use async_trait::async_trait;
use bincode::error::IntegerType::U128;
use bincode::{Decode, Encode};
use coingecko::response::coins::CoinsMarketItem;
use ethers::types::{Transaction, H160, H256, I256, U256};
use ethers_providers::{Provider, Ws};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::*;
use std::ops::Div;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
pub static CHECKED_COIN: Lazy<String> = Lazy::new(|| {
    std::env::var("ETH_CHECKED_COIN")
        .unwrap_or("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".to_string())
});
static IPC_PATH: Lazy<String> = Lazy::new(|| {
    std::env::var("ETH_IPC_PATH").unwrap_or_else(|_| {
        std::env::args()
            .nth(6)
            .unwrap_or("/root/.ethereum/geth.ipc".to_string())
    })
});
pub const POLL_INTERVAL: u64 = 60;
use crate::node_dispatcher::NodeDispatcher;
use crate::sushiswap::SushiSwapMetadata;
use nom::FindSubstring;
use num_bigfloat::{BigFloat, RoundingMode};
use rmps::{Deserializer, Serializer};
use tokio::runtime::Runtime;
use tracing::info;
use crate::curve::CurvePlainMetadata;
use crate::LiquidityProviderId::LuaSwap;
use crate::LiquidityProviders::ElcSwap;

#[async_trait]
pub trait LiquidityProvider:
    EventEmitter<Box<dyn EventSource<Event = PoolUpdateEvent>>>
    + EventEmitter<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>
{
    type Metadata;
    fn get_metadata(&self) -> Self::Metadata;
    async fn get_pools(&self) -> HashMap<String, Pool>;
    async fn set_pools(&self, pools: HashMap<String, Pool>);
    fn set_update_pools(&mut self, pools: Vec<[Arc<RwLock<Pool>>; 2]>);
    fn load_pools(&self, filter_tokens: Vec<String>) -> JoinHandle<()>;
    fn get_id(&self) -> LiquidityProviderId;
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LiquidityProviders {
    UniswapV2(UniswapV2Metadata),
    UniswapV3(UniswapV3Metadata),
    SushiSwap(UniswapV2Metadata),
    BalancerWeighted(BalancerWeigtedMetadata),
    Solidly(UniswapV2Metadata),
    Pancakeswap(UniswapV2Metadata),
    CroSwap(UniswapV2Metadata),
    ShibaSwap(UniswapV2Metadata),
    SaitaSwap(UniswapV2Metadata),
    ConvergenceSwap(UniswapV2Metadata),
    CurvePlain(CurvePlainMetadata),
    FraxSwap(UniswapV2Metadata),
    WiseSwap(UniswapV2Metadata),
    UnicSwap(UniswapV2Metadata),
    ApeSwap(UniswapV2Metadata),
    DXSwap(UniswapV2Metadata),
    LuaSwap(UniswapV2Metadata),
    ElcSwap(UniswapV2Metadata),
    CapitalSwap(UniswapV2Metadata),

}

#[derive(Deserialize, Serialize, Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
pub enum LiquidityProviderId {
    UniswapV2,
    UniswapV3,
    SushiSwap,
    BalancerWeighted,
    Solidly,
    Pancakeswap,
    CroSwap,
    ShibaSwap,
    SaitaSwap,
    ConvergenceSwap,
    CurvePlain,
    FraxSwap,
    WiseSwap,
    UnicSwap,
    ApeSwap,
    DXSwap,
    LuaSwap,
    ElcSwap,
    CapitalSwap,
}

impl<T: Into<String>> From<T> for LiquidityProviders {
    fn from(value: T) -> Self {
        match value.into().as_str() {
            "1" => LiquidityProviders::UniswapV2(Default::default()),
            "2" => LiquidityProviders::UniswapV3(UniswapV3Metadata {
                sqrt_price: U256::zero(),
                ..Default::default()
            }),
            "3" => LiquidityProviders::SushiSwap(Default::default()),
            "4" => LiquidityProviders::BalancerWeighted(Default::default()),
            "5" => LiquidityProviders::Solidly(Default::default()),
            "6" => LiquidityProviders::Pancakeswap(Default::default()),
            "7" => LiquidityProviders::CroSwap(Default::default()),
            "8" => LiquidityProviders::ShibaSwap(Default::default()),
            "9" => LiquidityProviders::SaitaSwap(Default::default()),
            "10" => LiquidityProviders::ConvergenceSwap(Default::default()),
            "11" => LiquidityProviders::CurvePlain(Default::default()),
            "12" => LiquidityProviders::FraxSwap(Default::default()),
            "13" => LiquidityProviders::WiseSwap(Default::default()),
            "14" => LiquidityProviders::UnicSwap(Default::default()),
            "15" => LiquidityProviders::ApeSwap(Default::default()),
            "16" => LiquidityProviders::DXSwap(Default::default()),
            "17" => LiquidityProviders::LuaSwap(Default::default()),
            "18" => LiquidityProviders::ElcSwap(Default::default()),
            "19" => LiquidityProviders::CapitalSwap(Default::default()),
            val => serde_json::from_str(val).unwrap(),

        }
    }
}

impl From<LiquidityProviders> for u8 {
    fn from(value: LiquidityProviders) -> Self {
        match value {
            LiquidityProviders::UniswapV2(_) => 1,
            LiquidityProviders::UniswapV3(_) => 2,
            LiquidityProviders::SushiSwap(_) => 3,
            LiquidityProviders::BalancerWeighted(_) => 4,
            LiquidityProviders::Solidly(_) => 5,
            LiquidityProviders::Pancakeswap(_) => 6,
            LiquidityProviders::CroSwap(_) => 7,
            LiquidityProviders::ShibaSwap(_) => 8,
            LiquidityProviders::SaitaSwap(_) => 9,
            LiquidityProviders::ConvergenceSwap(_) => 10,
            LiquidityProviders::CurvePlain(_) => 11,
            LiquidityProviders::FraxSwap(_) => 12,
            LiquidityProviders::WiseSwap(_) => 13,
            LiquidityProviders::UnicSwap(_) => 14,
            LiquidityProviders::ApeSwap(_) => 15,
            LiquidityProviders::DXSwap(_) => 16,
            LiquidityProviders::LuaSwap(_) => 17,
            LiquidityProviders::ElcSwap(_) => 18,
            LiquidityProviders::CapitalSwap(_) => 19,
        }
    }
}
pub trait EventSource: Send {
    type Event: Send;
    fn get_event(&self) -> Self::Event;
}
pub type BoxedLiquidityProvider =
    Box<dyn LiquidityProvider<Metadata = Box<dyn Meta>> + Send + Sync>;

impl LiquidityProviders {
    pub fn pay_address_signature(&self, inline: bool) -> String {
        match self {

            LiquidityProviders::UniswapV3(_) => "00000900".to_string(),
            LiquidityProviders::BalancerWeighted(_) => "00040000".to_string(),
            _ => {
                if inline {
                    "00006000".to_string()
                } else {
                    "00005000".to_string()
                }
            }
        }
    }
    pub fn pay_next_signature(&self, inline: bool) -> String {
        match self {

            LiquidityProviders::UniswapV3(_) => "00000800".to_string(),
            LiquidityProviders::BalancerWeighted(_) => "00030000".to_string(),
            _ => {
                if inline {
                    "00008000".to_string()

                } else {
                    "00007000".to_string()

                }
            }
        }
    }
    pub fn pay_self_signature(&self, inline: bool) -> String {
        match self {
            LiquidityProviders::UniswapV3(_) => "00000600".to_string(),
            LiquidityProviders::BalancerWeighted(_) => "00010000".to_string(),
            _ => {
                if inline {
                    "00004000".to_string()

                } else {
                    "00003000".to_string()

                }
            }
        }
    }
    pub fn pay_sender_signature(&self, inline: bool) -> String {
        match self {
            LiquidityProviders::UniswapV3(_) => "00000700".to_string(),
            LiquidityProviders::BalancerWeighted(_) => "00020000".to_string(),
            _ => {
                if inline {
                    "00002000".to_string()

                } else {
                    "00001000".to_string()

                }
            }
        }
    }
    pub fn build_calculator(&self) -> Box<dyn Calculator> {
        match self {
            LiquidityProviders::CurvePlain(meta) => Box::new(CurvePlainCalculator::new(meta.clone())),
            LiquidityProviders::UniswapV2(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::CroSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::ShibaSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::ConvergenceSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::SaitaSwap(meta) => Box::new(CpmmCalculator::new(20)),
            LiquidityProviders::Solidly(meta) => Box::new(CpmmCalculator::new(20)),
            LiquidityProviders::Pancakeswap(meta) => Box::new(CpmmCalculator::new(25)),
            LiquidityProviders::UniswapV3(meta) => Box::new(UniswapV3Calculator::new(meta.clone())),
            LiquidityProviders::SushiSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::FraxSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::WiseSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::UnicSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::ApeSwap(meta) => Box::new(CpmmCalculator::new(20)),
            LiquidityProviders::DXSwap(meta) => Box::new(CpmmCalculator::new(50)),
            LiquidityProviders::LuaSwap(meta) => Box::new(CpmmCalculator::new(40)),
            LiquidityProviders::ElcSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::CapitalSwap(meta) => Box::new(CpmmCalculator::new(30)),
            LiquidityProviders::BalancerWeighted(meta) => {
                Box::new(BalancerWeightedCalculator::new(meta.clone()))
            }

            _ => panic!("Invalid liquidity provider"),
        }
    }
    pub fn build(&self, node_providers: NodeDispatcher) -> BoxedLiquidityProvider {
        match self {
            LiquidityProviders::CapitalSwap(meta) => {
                // function uniswapV2Call(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0x03407772F5EBFB9B10Df007A2DD6FFf4EdE47B53".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::CapitalSwap,
                ))
            }
            LiquidityProviders::ElcSwap(meta) => {
                // function elkCall(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0x6511eBA915fC1b94b2364289CCa2b27AE5898d80".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::ElcSwap,
                ))
            }
            LiquidityProviders::LuaSwap(meta) => {
                // function uniswapV2Call(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0x0388C1E0f210AbAe597B7DE712B9510C6C36C857".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::LuaSwap,
                ))
            }
            LiquidityProviders::DXSwap(meta) => {
                // function DXswapCall(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0xd34971BaB6E5E356fd250715F5dE0492BB070452".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::DXSwap,
                ))
            }
            LiquidityProviders::ApeSwap(meta) => {
                // function apeCall(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0xBAe5dc9B19004883d0377419FeF3c2C8832d7d7B".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::ApeSwap,
                ))
            }
            LiquidityProviders::UnicSwap(meta) => {
                //  function unicSwapV2Call(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0xbAcC776b231c571a7e6ab7Bc2C8a099e07153377".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::UnicSwap,
                ))
            }
            LiquidityProviders::WiseSwap(meta) => {
                //  function swapsCall(address _sender, uint256 _amount0, uint256 _amount1, bytes calldata _data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0xee3E9E46E34a27dC755a63e2849C9913Ee1A06E2".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::WiseSwap,
                ))
            }
            LiquidityProviders::FraxSwap(meta) => {
                //  function uniswapV2Call(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0x43eC799eAdd63848443E2347C49f5f52e8Fe0F6f".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::FraxSwap,
                ))
            }
            LiquidityProviders::CurvePlain(meta) => {
                let metadata = CurvePlainMetadata {
                    factory_address: "0xF18056Bbd320E96A48e3Fbf8bC061322531aac99".to_string(),
                    ..meta.clone()
                };
                Box::new(curve::CurvePlain::new(
                    metadata,
                    node_providers,
                ))
            }
            LiquidityProviders::UniswapV2(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::UniswapV2,
                ))
            }
            LiquidityProviders::Solidly(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x777de5Fe8117cAAA7B44f396E93a401Cf5c9D4d6".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::Solidly,
                ))
            }
            LiquidityProviders::Pancakeswap(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x1097053Fd2ea711dad45caCcc45EfF7548fCB362".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::Pancakeswap,
                ))
            }
            LiquidityProviders::CroSwap(meta) => {
                //croDefiSwapCall(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0x9DEB29c9a4c7A88a3C0257393b7f3335338D9A9D".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::CroSwap,
                ))
            }
            LiquidityProviders::ShibaSwap(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0x115934131916C8b277DD010Ee02de363c09d037c".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::ShibaSwap,
                ))
            }
            LiquidityProviders::SaitaSwap(meta) => {
                // function SaitaSwapCall(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0x35113a300ca0D7621374890ABFEAC30E88f214b1".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::SaitaSwap,
                ))
            }
            LiquidityProviders::ConvergenceSwap(meta) => {
                //function swapCall(address sender, uint amount0, uint amount1, bytes calldata data) external;
                let metadata = UniswapV2Metadata {
                    factory_address: "0x4eef5746ED22A2fD368629C1852365bf5dcb79f1".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v2::UniSwapV2::new(
                    metadata,
                    node_providers,
                    LiquidityProviderId::ConvergenceSwap,
                ))
            }
            LiquidityProviders::UniswapV3(meta) => {
                let metadata = UniswapV3Metadata {
                    factory_address: "0x1F98431c8aD98523631AE4a59f267346ea31F984".to_string(),
                    ..meta.clone()
                };
                Box::new(uniswap_v3::UniSwapV3::new(metadata, node_providers))
            }
            LiquidityProviders::SushiSwap(meta) => {
                let metadata = UniswapV2Metadata {
                    factory_address: "0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac".to_string(),
                    ..meta.clone()
                };
                Box::new(sushiswap::SushiSwap::new(metadata, node_providers))
            }
            LiquidityProviders::BalancerWeighted(meta) => {
                let metadata = BalancerWeigtedMetadata {
                    factory_address: "0x8E9aa87E45e92bad84D5F8DD1bff34Fb92637dE9".to_string(),
                    ..meta.clone()
                };
                Box::new(balancer_weighted::BalancerWeighted::new(
                    metadata,
                    node_providers,
                ))
            }
            _ => panic!("Invalid liquidity provider"),
        }
    }

    pub fn id(&self) -> LiquidityProviderId {
        match self {
            LiquidityProviders::UniswapV2(_) => LiquidityProviderId::UniswapV2,
            LiquidityProviders::UniswapV3(_) => LiquidityProviderId::UniswapV3,
            LiquidityProviders::SushiSwap(_) => LiquidityProviderId::SushiSwap,
            LiquidityProviders::BalancerWeighted(_) => LiquidityProviderId::BalancerWeighted,
            LiquidityProviders::Solidly(_) => LiquidityProviderId::Solidly,
            LiquidityProviders::Pancakeswap(_) => LiquidityProviderId::Pancakeswap,
            LiquidityProviders::CroSwap(_) => LiquidityProviderId::CroSwap,
            LiquidityProviders::ShibaSwap(_) => LiquidityProviderId::ShibaSwap,
            LiquidityProviders::SaitaSwap(_) => LiquidityProviderId::SaitaSwap,
            LiquidityProviders::ConvergenceSwap(_) => LiquidityProviderId::ConvergenceSwap,
            LiquidityProviders::CurvePlain(_) => LiquidityProviderId::CurvePlain,
            LiquidityProviders::FraxSwap(_) => LiquidityProviderId::FraxSwap,
            LiquidityProviders::WiseSwap(_) => LiquidityProviderId::WiseSwap,
            LiquidityProviders::UnicSwap(_) => LiquidityProviderId::UnicSwap,
            LiquidityProviders::ApeSwap(_) => LiquidityProviderId::ApeSwap,
            LiquidityProviders::DXSwap(_) => LiquidityProviderId::DXSwap,
            LiquidityProviders::LuaSwap(_) => LiquidityProviderId::LuaSwap,
            LiquidityProviders::ElcSwap(_) => LiquidityProviderId::ElcSwap,
            LiquidityProviders::CapitalSwap(_) => LiquidityProviderId::CapitalSwap,
        }
    }

    pub fn factory_address(&self) -> String {
        match self {
            LiquidityProviders::UniswapV2(meta) => meta.factory_address.clone(),
            LiquidityProviders::UniswapV3(meta) => meta.factory_address.clone(),
            LiquidityProviders::SushiSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::BalancerWeighted(meta) => meta.factory_address.clone(),
            LiquidityProviders::Solidly(meta) => meta.factory_address.clone(),
            LiquidityProviders::Pancakeswap(meta) => meta.factory_address.clone(),
            LiquidityProviders::CroSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::ShibaSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::SaitaSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::ConvergenceSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::CurvePlain(meta) => meta.factory_address.clone(),
            LiquidityProviders::FraxSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::WiseSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::UnicSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::ApeSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::DXSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::LuaSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::ElcSwap(meta) => meta.factory_address.clone(),
            LiquidityProviders::CapitalSwap(meta) => meta.factory_address.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub enum Curve {
    Uncorrelated,
    Stable,
}

pub trait PoolInfo {
    fn getX(&self) -> String;
    fn getY(&self) -> String;
    fn supports_callback_payment(&self) -> bool;
    fn supports_pre_payment(&self) -> bool;
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq)]
pub struct Pool {
    pub address: String,
    pub x_address: String,
    pub y_address: String,
    pub curve: Option<String>,
    pub curve_type: Curve,
    pub fee_bps: u64,
    pub x_amount: U256,
    pub y_amount: U256,
    pub x_to_y: bool,
    pub provider: LiquidityProviders,
}

impl PoolInfo for Pool {
    fn getX(&self) -> String {
        self.x_address.clone()
    }

    fn getY(&self) -> String {
        self.y_address.clone()
    }

    fn supports_callback_payment(&self) -> bool {
        match self.provider.id() {

            LiquidityProviderId::UniswapV3 => true,
            LiquidityProviderId::BalancerWeighted => false,
            LiquidityProviderId::CurvePlain => false,
            _ => true
        }
    }

    fn supports_pre_payment(&self) -> bool {
        match self.provider.id() {
            LiquidityProviderId::UniswapV3 => false,
            LiquidityProviderId::BalancerWeighted => false,
            LiquidityProviderId::CurvePlain => false,
            _ => true
        }
    }
}

impl Hash for Pool {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.address.hash(state);
        self.x_address.hash(state);
        self.y_address.hash(state);
    }
}

impl PartialEq for Pool {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
            && self.x_address == other.x_address
            && self.y_address == other.y_address
    }
}
impl EventSource for Pool {
    type Event = Self;
    fn get_event(&self) -> Self::Event {
        self.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolUpdateEvent {
    pub pool: Pool,
    pub block_number: u64,
    pub timestamp: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPoolUpdateEvent {
    pub pool: Pool,
    pub pending_tx: Transaction,
    pub timestamp: u128,
    pub block_number: u64,
}

impl EventSource for PoolUpdateEvent {
    type Event = Self;
    fn get_event(&self) -> Self::Event {
        self.clone()
    }
}
impl EventSource for PendingPoolUpdateEvent {
    type Event = Self;
    fn get_event(&self) -> Self::Event {
        self.clone()
    }
}
impl From<&Pool> for Pool {
    fn from(pool: &Pool) -> Self {
        Self {
            address: pool.address.clone(),
            x_address: pool.x_address.clone(),
            y_address: pool.y_address.clone(),
            curve: pool.curve.clone(),
            curve_type: pool.curve_type.clone(),
            x_amount: pool.x_amount,
            y_amount: pool.y_amount,
            x_to_y: pool.x_to_y,
            provider: pool.provider.clone(),
            fee_bps: pool.fee_bps,
        }
    }
}

impl Display for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = format!("Pool {{ \n\tProvider: {:?}\n\taddress: {}\n\tx_address: {}\n\ty_address: {}\n\tis_x_to_y: {}\n\t", self.provider.id(), self.address,self.x_address, self.y_address,  self.x_to_y);
        match &self.provider {
            LiquidityProviders::UniswapV3(meta) => {
                out += &("Sqrt Price: ".to_string() + &meta.sqrt_price.to_string() + "\n\t" + "Tick: " + &meta.tick.to_string() + "\n\t" + "Liquidity: " + &meta.liquidity.to_string() + "\n\t");
            }
            LiquidityProviders::BalancerWeighted(meta) => {
                for (i, token) in meta.tokens.iter().enumerate() {
                    out += &("Token".to_string() + &i.to_string() + ": " + &token.address + " " + &token.balance.to_string() + "\n\t")
                }
                out += "\n\t";
            }
            LiquidityProviders::UniswapV2(meta) |
            LiquidityProviders::CroSwap(meta) |
            LiquidityProviders::ShibaSwap(meta) |
            LiquidityProviders::ConvergenceSwap(meta) |
            LiquidityProviders::SaitaSwap(meta) |
            LiquidityProviders::Solidly(meta) |
            LiquidityProviders::Pancakeswap(meta) |
            LiquidityProviders::SushiSwap(meta) |
            LiquidityProviders::FraxSwap(meta) |
            LiquidityProviders::WiseSwap(meta) |
            LiquidityProviders::UnicSwap(meta) |
            LiquidityProviders::ApeSwap(meta) |
            LiquidityProviders::DXSwap(meta) |
            LiquidityProviders::LuaSwap(meta) |
            LiquidityProviders::ElcSwap(meta) |
            LiquidityProviders::CapitalSwap(meta) => {
                out += &("Reserve0: ".to_string() + &meta.reserve0.to_string() + "\n\t" + "Reserve1: " + &meta.reserve1.to_string() + "\n\t");

            }
            _ => {}
        }
        write!(f, "{}", out)

    }
}

impl CpmmCalculator {
    fn new(fee: u64) -> Self {
        Self {
            fee
        }
    }
}

impl Calculator for CpmmCalculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero() {
            return Err(Error::msg("Insufficient Liquidity"));
        }
        let amount_in_with_fee = in_.saturating_mul(U256::from(10000 - self.fee));
        let numerator = amount_in_with_fee
            .checked_mul(swap_destination_amount)
            .unwrap_or(U256::from(0));
        let denominator = ((swap_source_amount) * 10000) + amount_in_with_fee;
        let amount_out = numerator / denominator;
        if amount_out > swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"));
        }

        Ok(amount_out)
    }

    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero()
            || swap_destination_amount.is_zero()
            || out_ >= swap_destination_amount
        {
            return Err(Error::msg("Insufficient Liquidity"));
        }

        if let Some(numerator) = swap_source_amount.checked_mul(out_ * 10000) {
            let denominator = (swap_destination_amount - out_) * U256::from((10000 - self.fee) as u128);
            Ok((numerator / denominator) + 1)
        } else {
            Err(Error::msg("Multiplication Overflow"))
        }
    }
}

pub trait Calculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256>;
    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256>;
}
pub struct CpmmCalculator {
    fee: u64
}

pub struct UniswapV3Calculator {
    meta: UniswapV3Metadata,
}

pub struct BalancerWeightedCalculator {
    meta: BalancerWeigtedMetadata,
}

pub struct CurvePlainCalculator {
    meta: CurvePlainMetadata,
}


impl BalancerWeightedCalculator {
    pub fn new(meta: BalancerWeigtedMetadata) -> Self {
        Self { meta }
    }

    // TODO
    pub fn mul_up(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        (x * y).round(9, RoundingMode::Up)
    }

    pub fn mul_down(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        (x * y).round(9, RoundingMode::Down)
    }

    pub fn div_up(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        (x / y).round(9, RoundingMode::Up)
    }
    pub fn div_down(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        (x / y).round(9, RoundingMode::Down)
    }

    pub fn pow_up(&self, x: BigFloat, y: BigFloat) -> BigFloat {
        if y == BigFloat::from(1) {
            x
        } else if y == BigFloat::from(2) {
            self.mul_up(x, x)
        } else if y == BigFloat::from(4) {
            let square = self.mul_up(x, x);
            self.mul_up(square, square)
        } else {
            x.pow(&y)
        }
    }
}


impl CurvePlainCalculator {
    pub fn new(meta: CurvePlainMetadata) -> Self {
        Self {
            meta
        }
    }

    pub fn swap_to(
        &self,
        source_amount: U256,
        swap_source_amount: U256,
        swap_destination_amount: U256,
    ) -> Option<U256> {
        let y = self.compute_y(
            swap_source_amount.checked_add(source_amount)?,
            self.compute_d(swap_source_amount, swap_destination_amount)?,
        )?;
        let dy = swap_destination_amount.checked_sub(y)?.checked_sub(1.into())?;
        let dy_fee = self.meta.fee.checked_mul(dy)?.checked_div(U256::from(10).pow(U256::from(10)))?;

        let amount_swapped = dy.checked_sub(dy_fee)?;

        Some(amount_swapped)
    }


    fn compute_next_d(
        &self,
        amp_factor: U256,
        d_init: U256,
        d_prod: U256,
        sum_x: U256,
    ) -> Option<U256> {
        let ann = amp_factor.checked_mul(self.meta.tokens.len().into())?;
        let leverage = sum_x.checked_mul(ann.into())?;
        // d = (ann * sum_x + d_prod * n_coins) * d / ((ann - 1) * d + (n_coins + 1) * d_prod)
        let numerator = d_init.checked_mul(
            d_prod
                .checked_mul(self.meta.tokens.len().into())?
                .checked_add(leverage.into())?,
        )?;
        let denominator = d_init
            .checked_mul(ann.checked_sub(U256::from(1))?.into())?
            .checked_add(d_prod.checked_mul((self.meta.tokens.len() + 1).into())?)?;
        numerator.checked_div(denominator)
    }


    pub fn compute_d(&self, amount_a: U256, amount_b: U256) -> Option<U256> {
        let sum_x = amount_a.checked_add(amount_b)?; // sum(x_i), a.k.a S
        if sum_x == 0.into() {
            Some(0.into())
        } else {
            let amp_factor = self.meta.amp;
            let amount_a_times_coins = amount_a.checked_mul(self.meta.tokens.len().into())?;
            let amount_b_times_coins = amount_b.checked_mul(self.meta.tokens.len().into())?;

            let mut d_prev: U256;
            let mut d: U256 = sum_x.into();
            for _ in 0..256 {
                let mut d_prod = d;
                d_prod = d_prod
                    .checked_mul(d)?
                    .checked_div(amount_a_times_coins.into())?;
                d_prod = d_prod
                    .checked_mul(d)?
                    .checked_div(amount_b_times_coins.into())?;
                d_prev = d;
                d = self.compute_next_d(amp_factor, d, d_prod, sum_x)?;
                if d > d_prev {
                    if d.checked_sub(d_prev)? <= 1.into() {
                        break;
                    }
                } else if d_prev.checked_sub(d)? <= 1.into() {
                    break;
                }
            }

            Some(d)
        }
    }

    pub fn compute_y(&self, x: U256, d: U256) -> Option<U256> {
        let amp_factor = self.meta.amp;
        let ann = amp_factor.checked_mul(self.meta.tokens.len().into())?; // A * n ** n

        // sum' = prod' = x
        // c =  D ** (n + 1) / (n ** (2 * n) * prod' * A)
        let mut c = d
            .checked_mul(d)?
            .checked_div(x.checked_mul(self.meta.tokens.len().into())?.into())?;
        c = c
            .checked_mul(d)?
            .checked_div(ann.checked_mul(self.meta.tokens.len().into())?.into())?;
        // b = sum' - (A*n**n - 1) * D / (A * n**n)
        let b = d.checked_div(ann.into())?.checked_add(x.into())?;

        // Solve for y by approximating: y**2 + b*y = c
        let mut y_prev: U256;
        let mut y = d;
        for _ in 0..256 {
            y_prev = y;
            // y = (y * y + c) / (2 * y + b - d);
            let y_numerator = y.checked_pow(2.into())?.checked_add(c)?;
            let y_denominator = y.checked_mul(2.into())?.checked_add(b)?.checked_sub(d)?;
            y = y_numerator.checked_div(y_denominator)?;
            if y > y_prev {
                if y.checked_sub(y_prev)? <= 1.into() {
                    break;
                }
            } else if y_prev.checked_sub(y)? <= 1.into() {
                break;
            }
        }
        Some(y)
    }

}

impl Calculator for CurvePlainCalculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero() || swap_destination_amount.is_zero() {
            return Err(Error::msg("Insufficient Liquidity"));
        }
        if let Some(amount_out) = self.swap_to(in_, swap_source_amount, swap_destination_amount) {
            Ok(amount_out)
        } else {
            Err(Error::msg("Unexpected Error"))
        }
    }
    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if swap_source_amount.is_zero()
            || swap_destination_amount.is_zero()
            || out_ >= swap_destination_amount
        {
            return Err(Error::msg("Insufficient Liquidity"));
        }

        if let Some(amount_in) = self.swap_to(out_, swap_destination_amount, swap_destination_amount) {
            Ok(amount_in)
        } else {
            Err(Error::msg("Unexpected Error"))
        }
    }
}

impl UniswapV3Calculator {
    pub const MAX_SQRT_RATIO: &str = "1461446703485210103287273052203988822378723970342";
    pub const MIN_SQRT_RATIO: &str = "4295128739";
    pub fn new(meta: UniswapV3Metadata) -> Self {
        Self { meta }
    }
}
#[async_trait]
impl Calculator for UniswapV3Calculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };

        if pool.y_amount == U256::from(0) || pool.x_amount == U256::from(0) {
            return Err(Error::msg("Insufficient Liquidity"));
        }
        let amount_out = self.meta.simulate_swap(pool.x_to_y, in_, true)?;
        if amount_out > swap_destination_amount {
            return Err(Error::msg("Insufficient Liquidity"));
        }
        Ok(amount_out)
    }

    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (swap_source_amount, swap_destination_amount) = if pool.x_to_y {
            (pool.x_amount, pool.y_amount)
        } else {
            (pool.y_amount, pool.x_amount)
        };
        if pool.y_amount == U256::from(0)
            || pool.x_amount == U256::from(0)
            || out_ >= swap_destination_amount
        {
            return Err(Error::msg("Insufficient Liquidity"));
        }

        let amount_in = self.meta.simulate_swap(pool.x_to_y, out_, false)?;
        Ok(amount_in)
    }
}

#[async_trait]
impl Calculator for BalancerWeightedCalculator {
    fn calculate_out(&self, in_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (source_token, dest_token) = if pool.x_to_y {
            (
                self.meta.tokens.first().unwrap(),
                self.meta.tokens.last().unwrap(),
            )
        } else {
            (
                self.meta.tokens.last().unwrap(),
                self.meta.tokens.first().unwrap(),
            )
        };
        if pool.y_amount == U256::from(0) || pool.x_amount == U256::from(0) {
            return Err(Error::msg("Insufficient Liquidity"));
        }

        let bo = BigFloat::from_str(&dest_token.balance.to_string()).unwrap();
        let bi = BigFloat::from_str(&source_token.balance.to_string()).unwrap();
        let ai = BigFloat::from_str(&in_.to_string()).unwrap();
        let wo = BigFloat::from_str(&dest_token.weight.to_string()).unwrap();
        let wi = BigFloat::from_str(&source_token.weight.to_string()).unwrap();

        let denominator = bi + ai;
        let base = self.div_up(bi, denominator);
        let exponent = self.div_down(wi, wo);
        let power = self.pow_up(base, exponent);
        let amount_out = self.mul_down(bo, power.sub(&BigFloat::from(1)));
        let amount_out_with_fee = self.div_up(
            amount_out,
            BigFloat::from(1).sub(
                &BigFloat::from_str(&self.meta.swap_fee.to_string())
                    .unwrap()
                    .div(&BigFloat::from(10).pow(&BigFloat::from(18))),
            ),
        );

        if let Some(amount_out_uint) = amount_out_with_fee.to_u128() {
            Ok(U256::from(amount_out_uint))
        } else {
            Err(Error::msg("Casting Error"))
        }
    }

    fn calculate_in(&self, out_: U256, pool: &Pool) -> anyhow::Result<U256> {
        let (source_token, dest_token) = if pool.x_to_y {
            (
                self.meta.tokens.first().unwrap(),
                self.meta.tokens.last().unwrap(),
            )
        } else {
            (
                self.meta.tokens.last().unwrap(),
                self.meta.tokens.first().unwrap(),
            )
        };

        if pool.y_amount == U256::from(0)
            || pool.x_amount == U256::from(0)
            || out_ >= dest_token.balance
        {
            return Err(Error::msg("Insufficient Liquidity"));
        }

        let bo = BigFloat::from_str(&dest_token.balance.to_string()).unwrap();
        let bi = BigFloat::from_str(&source_token.balance.to_string()).unwrap();
        let ao = BigFloat::from_str(&out_.to_string()).unwrap();
        let wo = BigFloat::from_str(&dest_token.weight.to_string()).unwrap();
        let wi = BigFloat::from_str(&source_token.weight.to_string()).unwrap();

        let base = self.div_up(bo, bo.sub(&ao));
        let exponent = self.div_up(wo, wi);
        let power = self.pow_up(base, exponent);
        // info!("bo: {} bi: {} ao: {} base: {} exponent: {} power: {}", bo.to_u128().unwrap(), bi.to_u128().unwrap(), ao.to_u128().unwrap(), base, exponent, power);
        let amount_in = self.mul_up(bi, power.sub(&BigFloat::from(1)));
        let amount_in_with_fee = self.div_up(
            amount_in,
            BigFloat::from(1).sub(
                &BigFloat::from_str(&self.meta.swap_fee.to_string())
                    .unwrap()
                    .div(&BigFloat::from(10).pow(&BigFloat::from(18))),
            ),
        );
        if let Some(amount_in_uint) = amount_in_with_fee.to_u128() {
            Ok(U256::from(amount_in_uint))
        } else {
            Err(Error::msg("Casting Error"))
        }
    }
}
pub trait Meta {}

pub struct SyncConfig {
    pub providers: Vec<LiquidityProviders>,
    pub from_file: bool,
}

pub async fn start(
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    updated_q: kanal::AsyncSender<Box<dyn EventSource<Event = PoolUpdateEvent>>>,
    pending_updated_q: kanal::AsyncSender<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>,
    mut used_pools: tokio::sync::oneshot::Receiver<Vec<[Arc<RwLock<Pool>>; 2]>>,
    nodes: NodeDispatcher,
    config: SyncConfig,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let mut join_handles = vec![];
    let mut amms = vec![];
    let mut v2_added = false;
    for provider in config.providers {
        let mut amm = provider.build(nodes.clone());
        if !config.from_file {
            join_handles.push(amm.load_pools(vec![]));
        }
        amm.subscribe(updated_q.clone());
        amm.subscribe(pending_updated_q.clone());
        match amm.get_id()  {
            LiquidityProviderId::UniswapV3 | LiquidityProviderId::BalancerWeighted | LiquidityProviderId::CurvePlain | LiquidityProviderId::SushiSwap => {
                amms.push(amm);
            }
            _ => {
                if !v2_added {
                    amms.push(amm);
                    v2_added = true;
                }
            }
        }

    }

    let filter_tokens: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string("blacklisted_tokens.json").unwrap()).unwrap();
    let filter_pools: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string("blacklisted_pools.json").unwrap()).unwrap();
    // Make sure pools are loaded before starting the listener
    info!("Loading Pools... ");
    for handle in join_handles {
        handle.await?;
    }
    let mut loaded_pools = HashMap::new();
    for amm in &amms {
        let pools = amm.get_pools().await;
        if pools.len() == 0 {
            continue;
        }
        let keys: Vec<String> = pools.keys().cloned().collect();
        std::fs::write(format!("{:?}", amm.get_id()), keys.join("\n")).expect("");
        for (addr, pool) in pools {
            if filter_tokens.contains(&pool.x_address) || filter_tokens.contains(&pool.y_address) {
                continue;
            }
            if filter_pools.contains(&pool.address) {
                continue;
            }
            loaded_pools.insert(addr, pool);
        }
    }

    let mut pools = pools.write().await;
    *pools = loaded_pools;
    std::mem::drop(pools);

    Ok(tokio::spawn(async move {
        let used_pools = used_pools.await.unwrap();
        let mut emitters = vec![];

        for mut amm in amms {
            let mut my_update_pools = vec![];
            let mut my_pools = HashMap::new();

            for pools in &used_pools {
                let pool = pools[0].read().await;
                if pool.provider.id() == amm.get_id() {
                    my_pools.insert(pool.address.clone(), pool.clone());
                    my_update_pools.push(pools.clone());
                }
            }
            info!("{:?} tracking {} pools", amm.get_id(), my_update_pools.len());

            amm.set_update_pools(my_update_pools);
            amm.set_pools(my_pools).await;
            emitters
                .push(EventEmitter::<Box<dyn EventSource<Event = PoolUpdateEvent>>>::emit(&*amm));
            emitters.push(EventEmitter::<
                Box<dyn EventSource<Event = PendingPoolUpdateEvent>>,
            >::emit(&*amm));
        }
        for emitter in emitters {
            emitter.join().unwrap();
        }
    }))
}

#[cfg(test)]
mod tests {
    use ethers::providers::{Provider, Ws};
    use tokio::test;
    use super::*;
    #[test]
    async fn test_uniswap_v3_calculator() {
        let mut meta = UniswapV3Metadata {
            factory_address: "".to_string(),
            address: "0xad57b47a8b403b1d756e3c2253248b3039c9b3e0".to_string(),
            token_a: "0x767fe9edc9e0df98e07454847909b5e959d7ca0e".to_string(),
            token_b: "0x7e77dcb127f99ece88230a64db8d595f31f1b068".to_string(),
            token_a_decimals: 18,
            token_b_decimals: 18,
            fee: 10000,
            liquidity: U256::from(2935216657493028029013 as u128),
            sqrt_price: U256::from_dec_str("93069955451674004580333445571").unwrap(),
            tick: -59012,
            tick_spacing: 60,
            liquidity_net: I256::zero(),
            ..Default::default()
        };
        let eth_client = Arc::new(
                Provider::<Ws>::connect("ws://89.58.31.215:8546")
                .await
                .unwrap(),
        );
        let x_y = abi::uniswap_v3::get_uniswap_v3_tick_data_batch_request(
            &meta,
            meta.tick,
            true,
            150,
            None,
            eth_client.clone(),
        )
        .await
        .unwrap();
        let y_x = abi::uniswap_v3::get_uniswap_v3_tick_data_batch_request(
            &meta,
            meta.tick,
            false,
            150,
            None,
            eth_client.clone(),
        )
        .await
        .unwrap();
        meta.tick_bitmap_x_y = x_y;
        meta.tick_bitmap_y_x = y_x;
        let complete_meta = abi::uniswap_v3::get_complete_pool_data_batch_request(vec![H160::from_str(&meta.address).unwrap()], &eth_client).await.unwrap().first().unwrap().clone();
        let pool = Pool {
            address: "".to_string(),
            x_address: "0x33349b282065b0284d756f0577fb39c158f935e6 ".to_string(),
            y_address: "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2 ".to_string(),
            curve: None,
            curve_type: Curve::Uncorrelated,
            fee_bps: 0,
            x_amount: U256::from(113005855278874616752 as u128),
            y_amount: U256::from(113005855278874616752 as u128),
            x_to_y: true,
            provider: LiquidityProviders::UniswapV3(complete_meta.clone()),
        };

        let calculator = pool.provider.build_calculator();

        let in_ = calculator.calculate_in(U256::from(3490852256887603537 as u128), &pool).unwrap();
        println!("{} ", in_);
    }

    #[tokio::test]
    async fn test_curve_plain_calculator() {

        let eth_client = Arc::new(
            Provider::<Ws>::connect("ws://89.58.31.215:8546")
                .await
                .unwrap(),
        );

        let address = "0x9409280DC1e6D33AB7A8C6EC03e5763FB61772B5";
        let complete_meta = abi::curve::get_complete_pool_data_batch_request(vec![H160::from_str(address).unwrap()], &eth_client).await.unwrap().first().unwrap().clone();
        let pool = Pool {
            address: "".to_string(),
            x_address: complete_meta.tokens.get(0).unwrap().clone(),
            y_address: complete_meta.tokens.get(1).unwrap().clone(),
            curve: None,
            curve_type: Curve::Stable,
            fee_bps: 0,
            x_amount: complete_meta.balances.get(0).unwrap().clone(),
            y_amount: complete_meta.balances.get(1).unwrap().clone(),
            x_to_y: true,
            provider: LiquidityProviders::CurvePlain(complete_meta.clone()),
        };

        let calculator = pool.provider.build_calculator();

        let in_ = calculator.calculate_out(U256::from(1000000 as u128), &pool).unwrap();
        println!("{} ", in_);
    }
}
