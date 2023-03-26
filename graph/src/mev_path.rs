#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use ethers::types::{Address, Eip1559TransactionRequest, Transaction, H160, H256, U128, U64};
use garb_sync_eth::{
    uniswap_v2::UniswapV2Metadata, uniswap_v3::UniswapV3Metadata, LiquidityProviderId,
    LiquidityProviders, Pool, PoolInfo, UniswapV3Calculator,
};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
// helper trait to filter solutions of interest
use crate::backrun::Backrun;
use crate::MAX_SIZE;
use ethers::abi::{AbiEncode, ParamType, StateMutability, Token};
use ethers::types::transaction::eip2930::AccessList;
use ethers::types::{I256, U256};
use itertools::Itertools;
use tracing::{debug, error, info, trace, warn};

const BINARY_SEARCH_ITERS: usize = 12;

const MINIMUM_PATH_LENGTH: usize = 2;
const UNISWAP_V3_EXACT_OUT_PAY_TO_SENDER: &str = "00002000";
const UNISWAP_V3_EXACT_IN_PAY_TO_SENDER: &str = "000000d0";
const UNISWAP_V3_EXACT_OUT_PAY_TO_SELF: &str = "00000600";
const UNISWAP_V3_EXACT_IN_PAY_TO_SELF: &str = "000000fc";
const UNISWAP_V3_EXACT_OUT_PAY_TO_ADDRESS: &str = "000000c9";
const UNISWAP_V3_EXACT_IN_PAY_TO_ADDRESS: &str = "00000091";

const UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER: &str = "0000000e";
const UNISWAP_V2_EXACT_IN_PAY_TO_SENDER: &str = "000000cd";
const UNISWAP_V2_EXACT_OUT_PAY_TO_SELF: &str = "0e000000";
const UNISWAP_V2_EXACT_IN_PAY_TO_SELF: &str = "00000082";
const UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS: &str = "000000e5";
const UNISWAP_V2_EXACT_IN_PAY_TO_ADDRESS: &str = "00000059";

const BALANCER_EXACT_OUT_PAY_TO_SENDER: &str = "20000000";
const BALANCER_EXACT_OUT_PAY_TO_SELF: &str = "10000000";

const PAY_ADDRESS: &str = "00000090";
const PAY_NEXT: &str = "00000010";
const PAY_SENDER: &str = "00000080";

fn hash_to_function_name(hash: &String) -> String {
    match hash.as_str() {
        "00002000" => "uniswapV3ExactOutPayToSender_A729BB".to_string(),
        "000000d0" => "uniswapV3ExactInPayToSender_1993B5C".to_string(),
        "00000600" => "uniswapV3ExactOutPayToSelf_1377F03".to_string(),
        "000000fc" => "uniswapV3ExactInPayToSelf_A9C0BD".to_string(),
        "000000c9" => "uniswapV3ExactOutPayToAddress_37EB331".to_string(),
        "00000091" => "uniswapV3ExactInPayToAddress_8F71A6".to_string(),

        "0000000e" => "uniswapV2ExactOutPayToSender_31D5F3".to_string(),
        "000000cd" => "uniswapV2ExactInPayToSender_120576".to_string(),
        "0e000000" => "uniswapV2ExactOutPayToSelf_12BAA3".to_string(),
        "00000082" => "uniswapV2ExactInPayToSelf_FDC770".to_string(),
        "000000e5" => "uniswapV2ExactOutPayToAddress_E0E335".to_string(),
        "00000059" => "uniswapV2ExactInPayToAddress_35CB03".to_string(),

        "00000081" => "payAddress_1A718EA".to_string(),
        "00000080" => "paySender_7437EA".to_string(),
        _ => "".to_string(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct MevPath {
    pub locked_pools: Vec<Arc<RwLock<Pool>>>,
    pub pools: Vec<Pool>,
    pub input_token: String,
    pub optimal_path: PathKind
}

fn sub_i256(first: I256, second: I256) -> I256 {
    if first > second {
        let diff = first.abs() - second.abs();
        if first < I256::from(0) {
            -diff
        } else {
            diff
        }
    } else {
        let diff = second.abs() - first.abs();

        -(diff)
    }
}
#[derive(Debug, Clone)]
pub struct StepMeta {
    pub step_id: String,
    pub asset: I256,
    pub debt: I256,
    pub asset_token: String,
    pub debt_token: String,
    pub step: Pool,
}

#[derive(Debug, Clone)]
pub struct PathResult {
    pub ix_data: String,
    pub profit: u128,
    pub is_good: bool,
    pub steps: Vec<StepMeta>,
}
impl Default for PathResult {
    fn default() -> Self {
        Self {
            ix_data: "".to_string(),
            profit: 0,
            is_good: false,
            steps: vec![],
        }
    }
}

impl MevPath {
    pub fn new(
        pools: Vec<Pool>,
        pools_locked: &Vec<Arc<RwLock<Pool>>>,
        input_token: &String,
    ) -> Self {

            Self {
                input_token: input_token.clone(),
                locked_pools: pools_locked.clone(),
                pools: pools.clone(),
                optimal_path: PathKind::SCSP
            }

    }


    fn two_step_scsn_sync(&self, first: &Pool, second: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];

        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();

        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);


            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let final_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else {
                continue;
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }

            let (asset_token, debt_token) = if first.x_to_y {
                (first.y_address.clone(), first.x_address.clone())
            } else {
                (first.x_address.clone(), first.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsn_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                + &(packed_asset.len() as u8).encode_hex()[64..]
                + &packed_asset);


            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            instruction.push(ix);
            let packed_debt = Self::encode_packed_uint(final_debt);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsn_3".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = second.provider.pay_sender_signature(false);
            let packed_asset = Self::encode_packed_uint(first_debt);
            // second is guaranteed to be balancer pools

            match &second.provider {
                LiquidityProviders::BalancerWeighted(meta) => {
                    ix += &(meta.id[2..].to_string()
                            + &debt_token[2..]
                            + &asset_token[2..]
                            + &(packed_asset.len() as u8).encode_hex()[64..]
                            + &packed_asset
                            + &(packed_debt.len() as u8).encode_hex()[64..]
                            + &packed_debt)
                }
                // only balancer pools support niether so this should never match
                _ => {}
            }
            instruction.push(ix);

            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }
        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSN)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }

    fn two_step_scsc_sync(&self, first: &Pool, second: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);


            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let final_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else {
                continue;
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }


            let (asset_token, debt_token) = if first.x_to_y {
                (first.y_address.clone(), first.x_address.clone())
            } else {
                (first.x_address.clone(), first.y_address.clone())
            };

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsc_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                + &(packed_asset.len() as u8).encode_hex()[64..]
                + &packed_asset);

            instruction.push(ix);
            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsc_2".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = second.provider.pay_sender_signature(false) + &second.address[2..];
            let packed_asset = Self::encode_packed_uint(first_debt);
            let packed_debt = Self::encode_packed_uint(final_debt);

            // second is guaranteed to be v3 pools
            // since v2 pools will be matched by scsp
            ix += &(if second.x_to_y { "01".to_string() } else { "00".to_string() }
                + &(packed_asset.len() as u8).encode_hex()[64..]
                + &packed_asset);
            instruction.push(ix);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsc_3".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = PAY_SENDER.to_string()
                + &debt_token[2..]
                + &(packed_debt.len() as u8).encode_hex()[64..]
                + &packed_debt;

            instruction.push(ix);

            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }
        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSC)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }

    fn two_step_scsp_sync(&self, first: &Pool, second: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);


            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let final_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else {
                continue;
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }



            let (asset_token, debt_token) = if first.x_to_y {
                (first.y_address.clone(), first.x_address.clone())
            } else {
                (first.x_address.clone(), first.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsp_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                + &(packed_asset.len() as u8).encode_hex()[64..]
            + &packed_asset);
            instruction.push(ix);
            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            let packed_debt = Self::encode_packed_uint(final_debt);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsp_2".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = PAY_NEXT.to_string()
                + &debt_token[2..]
                + &(packed_debt.len() as u8).encode_hex()[64..]
                + &packed_debt;

            instruction.push(ix);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsp_2".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: second.clone(),
            });
            let mut ix = second.provider.pay_sender_signature(true) + &second.address[2..];
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if second.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSP)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }

    fn three_step_scspsp_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);


            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }

            let (asset_token, debt_token) = if first.x_to_y {
                (first.y_address.clone(), first.x_address.clone())
            } else {
                (first.x_address.clone(), first.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsp_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);


            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };

            let packed_debt = Self::encode_packed_uint(final_debt);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsp_2".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = PAY_NEXT.to_string()
                         + &debt_token[2..]
                         + &(packed_debt.len() as u8).encode_hex()[64..]
                         + &packed_debt;

            instruction.push(ix);
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsp_3".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: third.clone(),
            });
            let mut ix = third.provider.pay_next_signature(true) + &third.address[2..];
            // third is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(second_debt);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsp_3".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: second.clone(),
            });
            let mut ix = second.provider.pay_address_signature(true) + &second.address[2..];
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(first_debt);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &first.address[2..]
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);
            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSPSP)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }

    fn three_step_scspsc_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsc_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsc_2".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = third.provider.pay_next_signature(false) + &third.address[2..];
            // third is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(second_debt);
            ix += &(if third.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let packed_debt = Self::encode_packed_uint(final_debt);


            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsc_3".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = second.provider.pay_address_signature(true) + &second.address[2..];
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(first_debt);
            ix += &(if second.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &first.address[2..]
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsc_4".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = PAY_SENDER.to_string()
                + &debt_token[2..]
                + &(packed_debt.len() as u8).encode_hex()[64..]
                + &packed_debt;

            instruction.push(ix);

            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSPSC)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }

    fn three_step_scspsn_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsn_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsn_2".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = third.provider.pay_next_signature(false);
            // third is guaranteed to be balancer
            let packed_asset = Self::encode_packed_uint(second_debt);
            let packed_debt = Self::encode_packed_uint(final_debt);
            match &third.provider {
                LiquidityProviders::BalancerWeighted(meta) => {
                    ix += &(meta.id[2..].to_string()
                            + &debt_token[2..]
                            + &asset_token[2..]
                            + &(packed_asset.len() as u8).encode_hex()[64..]
                            + &packed_asset
                            + &(packed_debt.len() as u8).encode_hex()[64..]
                            + &packed_debt)
                }
                // only balancer pools support niether so this should never match
                _ => {}
            }
            instruction.push(ix);

            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scspsn_3".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: second.clone(),
            });
            let mut ix = second.provider.pay_address_signature(true) + &second.address[2..];
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(first_debt);
            ix += &(if second.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &first.address[2..]
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);


            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSPSN)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }


    fn three_step_scscsp_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if first.x_to_y {
                (first.y_address.clone(), first.x_address.clone())
            } else {
                (first.x_address.clone(), first.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsp_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsp_2".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = second.provider.pay_sender_signature(false) + &second.address[2..];
            // third is guaranteed to be balancer
            let packed_asset = Self::encode_packed_uint(first_debt);
            ix += &(if second.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };

            let packed_debt = Self::encode_packed_uint(final_debt);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsp_3".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = PAY_NEXT.to_string()
                         + &debt_token[2..]
                         + &(packed_debt.len() as u8).encode_hex()[64..]
                         + &packed_debt;

            instruction.push(ix);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsp_4".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: third.clone(),
            });
            let mut ix = third.provider.pay_sender_signature(true) + &third.address[2..];
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(second_debt);
            ix += &(if third.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);


            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSCSP)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }


    fn three_step_scscsc_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };

            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else {
                continue
            };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if first.x_to_y {
                (first.y_address.clone(), first.x_address.clone())
            } else {
                (first.x_address.clone(), first.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsc_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsc_2".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = second.provider.pay_sender_signature(false) + &second.address[2..];
            // third is guaranteed to be balancer
            let packed_asset = Self::encode_packed_uint(first_debt);
            ix += &(if second.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };



            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsc_3".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = third.provider.pay_sender_signature(false) + &third.address[2..];
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(second_debt);
            ix += &(if third.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);
            let packed_debt = Self::encode_packed_uint(final_debt);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsc_4".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = PAY_SENDER.to_string()
                         + &debt_token[2..]
                         + &(packed_debt.len() as u8).encode_hex()[64..]
                         + &packed_debt;

            instruction.push(ix);

            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSCSC)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }


    fn three_step_scscsn_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if first.x_to_y {
                (first.y_address.clone(), first.x_address.clone())
            } else {
                (first.x_address.clone(), first.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsn_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsn_2".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = second.provider.pay_sender_signature(false) + &second.address[2..];
            // third is guaranteed to be balancer
            let packed_asset = Self::encode_packed_uint(first_debt);
            ix += &(if second.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };



            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsn_3".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = third.provider.pay_sender_signature(false);
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(second_debt);
            let packed_debt = Self::encode_packed_uint(final_debt);
            match &third.provider {
                LiquidityProviders::BalancerWeighted(meta) => {
                    ix += &(meta.id[2..].to_string()
                            + &debt_token[2..]
                            + &asset_token[2..]
                            + &(packed_asset.len() as u8).encode_hex()[64..]
                            + &packed_asset
                            + &(packed_debt.len() as u8).encode_hex()[64..]
                            + &packed_debt)
                }
                // only balancer pools support niether so this should never match
                _ => {}
            }
            instruction.push(ix);


            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSCSN)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }


    fn three_step_scsnsp_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsp_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };

            let packed_debt = Self::encode_packed_uint(final_debt);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsp_2".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = PAY_NEXT.to_string()
                         + &debt_token[2..]
                         + &(packed_debt.len() as u8).encode_hex()[64..]
                         + &packed_debt;

            instruction.push(ix);



            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsp_3".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = third.provider.pay_self_signature(true) + &third.address[2..];
            // third is guaranteed to be balancer
            let packed_asset = Self::encode_packed_uint(second_debt);
            ix += &(if third.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);

            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };



            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsp_4".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });

            // second is guaranteed to be balancer
            let mut ix = second.provider.pay_address_signature(false);
            let packed_asset = Self::encode_packed_uint(first_debt);
            let packed_debt = Self::encode_packed_uint(second_debt);
            match &second.provider {
                LiquidityProviders::BalancerWeighted(meta) => {
                    ix += &(meta.id[2..].to_string()
                            + &first.address
                            + &debt_token[2..]
                            + &asset_token[2..]
                            + &(packed_asset.len() as u8).encode_hex()[64..]
                            + &packed_asset
                            + &(packed_debt.len() as u8).encode_hex()[64..]
                            + &packed_debt)
                }
                // only balancer pools support niether so this should never match
                _ => {}
            }
            instruction.push(ix);


            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSNSP)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }


    fn three_step_scsnsc_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsc_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsc_2".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = third.provider.pay_sender_signature(false) + &third.address[2..];
            // third is guaranteed to be balancer
            let packed_asset = Self::encode_packed_uint(second_debt);
            ix += &(if third.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);


            instruction.push(ix);
            let packed_debt = Self::encode_packed_uint(final_debt);

            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsc_3".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = PAY_SENDER.to_string()
                         + &debt_token[2..]
                         + &(packed_debt.len() as u8).encode_hex()[64..]
                         + &packed_debt;

            instruction.push(ix);
            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (second.x_address.clone(), second.y_address.clone())
            };



            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scsnsc_4".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = second.provider.pay_address_signature(false);
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(first_debt);
            let packed_debt = Self::encode_packed_uint(second_debt);
            match &second.provider {
                LiquidityProviders::BalancerWeighted(meta) => {
                    ix += &(meta.id[2..].to_string()
                            + &first.address
                            + &debt_token[2..]
                            + &asset_token[2..]
                            + &(packed_asset.len() as u8).encode_hex()[64..]
                            + &packed_asset
                            + &(packed_debt.len() as u8).encode_hex()[64..]
                            + &packed_debt)
                }
                // only balancer pools support niether so this should never match
                _ => {}
            }
            instruction.push(ix);


            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSNSC)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }


    fn three_step_scsnsn_sync(&self, first: &Pool, second: &Pool, third: &Pool) -> anyhow::Result<PathResult> {
        let decimals = crate::decimals(self.input_token.clone());
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 20.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let calc1 = first.provider.build_calculator();
        let calc2 = second.provider.build_calculator();
        let calc3 = third.provider.build_calculator();
        'binary_search: for i in 0..BINARY_SEARCH_ITERS {
            let mut steps = vec![];
            let mut instruction = vec![];
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = U256::from(i_atomic as u128);
            let first_debt = if let Ok(x) = calc1.calculate_in(asset, first) {
                x
            } else {
                continue
            };
            let second_debt = if let Ok(x) = calc2.calculate_in(first_debt, second) {
                x
            } else { continue };

            let final_debt = if let Ok(x) = calc3.calculate_in(second_debt, third) {
                x
            } else {
                continue
            };

            let final_balance = sub_i256(I256::from_raw(asset), I256::from_raw(final_debt));
            if final_debt > asset {
                best_route_profit = final_balance;
                right = mid;
                mid = (left + right) / 2.0;
                continue
            }
            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };


            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsn_1".to_string(),
                asset: I256::from_raw(asset),
                debt: I256::from_raw(first_debt),
                asset_token: asset_token,
                debt_token: debt_token,
                step: first.clone(),
            });

            let mut ix = first.provider.pay_self_signature(false) + &first.address[2..];
            // first is guaranteed to be either v3 pools or v2 variants
            let packed_asset = Self::encode_packed_uint(asset);
            ix += &(if first.x_to_y { "01".to_string() } else { "00".to_string() }
                    + &(packed_asset.len() as u8).encode_hex()[64..]
                    + &packed_asset);
            instruction.push(ix);

            let (asset_token, debt_token) = if third.x_to_y {
                (third.y_address.clone(), third.x_address.clone())
            } else {
                (third.x_address.clone(), third.y_address.clone())
            };
            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsn_2".to_string(),
                asset: I256::from_raw(second_debt),
                debt: I256::from_raw(final_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: third.clone(),
            });
            let mut ix = third.provider.pay_sender_signature(false);
            // third is guaranteed to be balancer
            let packed_asset = Self::encode_packed_uint(second_debt);
            let packed_debt = Self::encode_packed_uint(final_debt);
            match &third.provider {
                LiquidityProviders::BalancerWeighted(meta) => {
                    ix += &(meta.id[2..].to_string()
                            + &debt_token[2..]
                            + &asset_token[2..]
                            + &(packed_asset.len() as u8).encode_hex()[64..]
                            + &packed_asset
                            + &(packed_debt.len() as u8).encode_hex()[64..]
                            + &packed_debt)
                }
                // only balancer pools support niether so this should never match
                _ => {}
            }


            instruction.push(ix);

            let (asset_token, debt_token) = if second.x_to_y {
                (second.y_address.clone(), second.x_address.clone())
            } else {
                (third.x_address.clone(), second.y_address.clone())
            };



            #[cfg(not(feature = "optimized"))]
            steps.push(StepMeta {
                step_id: "scscsn_3".to_string(),
                asset: I256::from_raw(first_debt),
                debt: I256::from_raw(second_debt),
                asset_token: asset_token.clone(),
                debt_token: debt_token.clone(),
                step: second.clone(),
            });
            let mut ix = third.provider.pay_sender_signature(false);
            // second is guaranteed to be v2 variants
            let packed_asset = Self::encode_packed_uint(first_debt);
            let packed_debt = Self::encode_packed_uint(second_debt);
            match &second.provider {
                LiquidityProviders::BalancerWeighted(meta) => {
                    ix += &(meta.id[2..].to_string()
                            + &debt_token[2..]
                            + &asset_token[2..]
                            + &(packed_asset.len() as u8).encode_hex()[64..]
                            + &packed_asset
                            + &(packed_debt.len() as u8).encode_hex()[64..]
                            + &packed_debt)
                }
                // only balancer pools support niether so this should never match
                _ => {}
            }
            instruction.push(ix);


            steps_meta.push(steps);
            instructions.push(instruction);
            if i == 0 {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else if final_balance >= best_route_profit {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                best_route_index = instructions.len() - 1;
                if best_route_profit > I256::from(0) {
                    left = mid;
                } else {
                    right = mid;
                }
            } else {
                best_route_profit = final_balance;
                best_route_size = i_atomic;
                right = mid;
            }
            mid = (left + right) / 2.0;
        }

        if best_route_profit == I256::zero() {
            Err(anyhow::Error::msg(format!("Invalid Path {:?}", PathKind::SCSNSN)))
        } else if best_route_profit > I256::from(0) {
            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(Default::default())
        }
    }


    fn chained_out_path_sync(&self, mut path: Vec<Pool>) -> anyhow::Result<PathResult> {
        // binary search for optimal input
        match self.optimal_path {
            PathKind::SCSP => {
                let first = path.first().unwrap();
                let second = path.last().unwrap();
                self.two_step_scsp_sync(first, second)
            },
            PathKind::SCSC => {
                let first = path.first().unwrap();
                let second = path.last().unwrap();
                self.two_step_scsc_sync(first, second)
            },
            PathKind::SCSN => {
                let first = path.first().unwrap();
                let second = path.last().unwrap();
                self.two_step_scsn_sync(first, second)
            }
            PathKind::SCSPSP => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scspsp_sync(first, second, third)
            }
            PathKind::SCSPSC => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scspsc_sync(first, second, third)
            }
            PathKind::SCSPSN => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scspsn_sync(first, second, third)
            }
            PathKind::SCSCSP => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scscsp_sync(first, second, third)
            }
            PathKind::SCSCSC => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scscsc_sync(first, second, third)
            }
            PathKind::SCSCSN => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scscsn_sync(first, second, third)
            }
            PathKind::SCSNSN => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scsnsn_sync(first, second, third)
            }
            PathKind::SCSNSP => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scsnsp_sync(first, second, third)
            }
            PathKind::SCSNSC => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                self.three_step_scsnsc_sync(first, second, third)
            }
        }
    }

    fn encode_int(amount: I256) -> String {
        if amount < I256::from(0) {
            (-amount).encode_hex()[2..].to_string()
        } else {
            amount.encode_hex()[2..].to_string()
        }
    }

    pub fn encode_packed(amount: I256) -> String {
        let encoded = if amount < I256::from(0) {
            (-amount).encode_hex()
        } else {
            amount.encode_hex()
        };
        let mut index = 0;
        for l in encoded.chars() {
            if l == '0' || (l == 'x' && index == 1) {
                index += 1;
                continue;
            } else {
                break;
            }
        }
        let data = if encoded[index..].len() % 2 == 0 {
            return encoded[index..].to_string();
        } else {
            return "0".to_string() + &encoded[index..];
        };
    }
    pub fn encode_packed_uint(amount: U256) -> String {
        let encoded = amount.encode_hex();
        let mut index = 0;
        for l in encoded.chars() {
            if l == '0' || (l == 'x' && index == 1) {
                index += 1;
                continue;
            } else {
                break;
            }
        }
    let data = if encoded[index..].len() % 2 == 0 {
            return encoded[index..].to_string();
        } else {
            return "0".to_string() + &encoded[index..];
        };
    }
    pub fn get_transaction_sync(
        &self,
        pools_path: Vec<Pool>,
    ) -> Option<(Eip1559TransactionRequest, PathResult)> {
        let is_good = self.chained_out_path_sync(pools_path);
        match &is_good {
            Ok(data) => {
                if !data.is_good {
                    return None;
                } else {
                    info!("{:?}: {}",self.optimal_path,  data.ix_data);
                    let tx_request = Eip1559TransactionRequest {
                        // update later
                        to: None,
                        // update later
                        from: None,
                        data: Some(ethers::types::Bytes::from_str(&data.ix_data).unwrap()),
                        chain_id: Some(U64::from(1)),
                        max_priority_fee_per_gas: None,
                        // update later
                        max_fee_per_gas: None,
                        gas: None,
                        // update later
                        nonce: None,
                        value: None,
                        access_list: AccessList::default(),
                    };
                    return Some((tx_request, data.clone()));
                }
            }
            Err(e) => {
                trace!("{:?}", e);
                None
            }
        }
    }

    pub fn process_path(
            &self,
        mut path: Vec<Pool>,
        input_token: &String,
        ) -> anyhow::Result<PathKind> {
        if path.len() <= 0 {
            return Err(anyhow::Error::msg("Path Too Short"))
        }
        let first = path.first().unwrap();
        if !first.supports_callback_payment() {
            return Err(anyhow::Error::msg("Invalid Path"));
        }

        // try from the most gas saving first
        match path.len() {
            0 | 1 => {
                return Err(anyhow::Error::msg("Path too short"));
            }
            2 => {
                // first pool always supports callback payment
                let first = path.first().unwrap();
                let second = path.last().unwrap();
                match second.supports_pre_payment() {
                    true => {
                        if let Ok(result) = self.two_step_scsp_sync(first, second) {
                            return Ok(PathKind::SCSP)
                        }
                    },
                    false => {
                        match second.supports_callback_payment() {
                            true => {
                                if let Ok(result) = self.two_step_scsc_sync(first, second) {
                                    return Ok(PathKind::SCSC)
                                }
                            }
                            false => {
                                if let Ok(result) = self.two_step_scsn_sync(first, second) {
                                    return Ok(PathKind::SCSN)
                                }
                            }
                        }
                    }
                }
                match second.supports_callback_payment() {
                    true => {
                        if let Ok(result) = self.two_step_scsc_sync(first, second) {
                            return Ok(PathKind::SCSC)
                        }
                    }
                    false => {
                        if let Ok(result) = self.two_step_scsn_sync(first, second) {
                            return Ok(PathKind::SCSN)
                        }
                    }
                }

            }
            3 => {
                let first = path.first().unwrap();
                let second = path.get(1).unwrap();
                let third = path.last().unwrap();
                match second.supports_pre_payment() {
                    true => {
                        match third.supports_pre_payment() {
                            true => {
                                if let Ok(result) = self.three_step_scspsp_sync(first, third, second) {
                                    return Ok(PathKind::SCSPSP)
                                }
                            }
                            false => {
                                match third.supports_callback_payment() {
                                    true => {
                                        if let Ok(result) = self.three_step_scspsc_sync(third, first, second) {
                                            return Ok(PathKind::SCSPSC)
                                        }
                                    } false => {
                                        if let Ok(result) = self.three_step_scspsn_sync(first, third, second) {
                                            return Ok(PathKind::SCSPSN)
                                        }
                                    }
                                }
                            }
                        }

                    }
                    false => {
                        match second.supports_callback_payment() {
                            true => {
                                match third.supports_pre_payment() {
                                    true => {
                                        if let Ok(result) = self.three_step_scscsp_sync(first, third, second) {
                                            return Ok(PathKind::SCSCSP)
                                        }
                                    }
                                    false => {
                                        match third.supports_callback_payment() {
                                            true => {
                                                if let Ok(result) = self.three_step_scscsc_sync(third, first, second) {
                                                    return Ok(PathKind::SCSCSC)
                                                }
                                            } false => {
                                                if let Ok(result) = self.three_step_scscsn_sync(first, third, second) {
                                                    return Ok(PathKind::SCSCSN)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            false => {
                                match third.supports_pre_payment() {
                                    true => {
                                        if let Ok(result) = self.three_step_scsnsp_sync(first, third, second) {
                                            return Ok(PathKind::SCSNSP)
                                        }
                                    }
                                    false => {
                                        match third.supports_callback_payment() {
                                            true => {
                                                if let Ok(result) = self.three_step_scsnsc_sync(third, first, second) {
                                                    return Ok(PathKind::SCSNSC)
                                                }
                                            } false => {
                                                if let Ok(result) = self.three_step_scsnsn_sync(first, third, second) {
                                                    return Ok(PathKind::SCSNSN)
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                        }
                    }

                }
                match second.supports_callback_payment() {
                    true => {
                        match third.supports_pre_payment() {
                            true => {
                                if let Ok(result) = self.three_step_scscsp_sync(first, third, second) {
                                    return Ok(PathKind::SCSCSP)
                                }
                            }
                            false => {
                                match third.supports_callback_payment() {
                                    true => {
                                        if let Ok(result) = self.three_step_scscsc_sync(third, first, second) {
                                            return Ok(PathKind::SCSCSC)
                                        }
                                    } false => {
                                        if let Ok(result) = self.three_step_scscsn_sync(first, third, second) {
                                            return Ok(PathKind::SCSCSN)
                                        }
                                    }
                                }
                            }
                        }
                    }
                    false => {
                        match third.supports_pre_payment() {
                            true => {
                                if let Ok(result) = self.three_step_scsnsp_sync(first, third, second) {
                                    return Ok(PathKind::SCSNSP)
                                }
                            }
                            false => {
                                match third.supports_callback_payment() {
                                    true => {
                                        if let Ok(result) = self.three_step_scsnsc_sync(third, first, second) {
                                            return Ok(PathKind::SCSNSC)
                                        }
                                    } false => {
                                        if let Ok(result) = self.three_step_scsnsn_sync(first, third, second) {
                                            return Ok(PathKind::SCSNSN)
                                        }
                                    }
                                }
                            }
                        }
                    }

                }
            }
            _ => return Err(anyhow::Error::msg("Invalid Path")),
        }

        Err(anyhow::Error::msg("Invalid Path"))
    }
}
#[derive(Debug, Clone, Default)]
pub enum PathKind {
    #[default]
    SCSP,
    SCSC,
    SCSN,
    SCSPSP,
    SCSPSC,
    SCSPSN,
    SCSCSP,
    SCSCSC,
    SCSCSN,
    SCSNSN,
    SCSNSP,
    SCSNSC,
}
