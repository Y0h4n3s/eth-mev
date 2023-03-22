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

const PAY_ADDRESS: &str = "00000081";
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
    pub path: Vec<MevPathStep>,
    pub pools: Vec<Pool>,
    pub input_token: String,
}

#[derive(Debug, Clone, Default)]
pub struct StepInput {
    pub function_hash: String,
    pub pay_to: String,
    pub amount: u128,
}

#[derive(Debug, Clone, Default)]
pub struct StepOutput {
    target: String,
}

#[derive(Debug, Clone)]
pub enum MevPathStep {
    ExactIn(Arc<RwLock<Pool>>, StepInput, StepOutput),
    ExactOut(Arc<RwLock<Pool>>, StepInput, StepOutput),
    // pool: the pool to pay back
    // bool: the token we're paying back, true for x false for y
    Payback(Arc<RwLock<Pool>>, StepInput, bool),
}

impl MevPathStep {
    pub fn update_input(&mut self, _in: &StepInput) {
        match self {
            MevPathStep::ExactIn(_, input, _)
            | MevPathStep::ExactOut(_, input, _)
            | MevPathStep::Payback(_, input, _) => {
                *input = _in.clone();
            }
        }
    }
    pub fn update_output(&mut self, out: &StepOutput) {
        match self {
            MevPathStep::ExactIn(_, _, output) | MevPathStep::ExactOut(_, _, output) => {
                *output = out.clone();
            }
            MevPathStep::Payback(_, _, _) => (),
        }
    }

    pub async fn get_pool(&self) -> Pool {
        match self {
            MevPathStep::ExactIn(p, _, _)
            | MevPathStep::ExactOut(p, _, _)
            | MevPathStep::Payback(p, _, _) => p.read().await.clone(),
        }
    }

    pub fn get_pool_arc(&self) -> Arc<RwLock<Pool>> {
        match self {
            MevPathStep::ExactIn(p, _, _)
            | MevPathStep::ExactOut(p, _, _)
            | MevPathStep::Payback(p, _, _) => p.clone(),
        }
    }

    pub fn get_output(&self) -> String {
        match self {
            MevPathStep::ExactIn(_, _, o) | MevPathStep::ExactOut(_, _, o) => o.target.clone(),
            MevPathStep::Payback(p, _, _) => ".".to_string(),
        }
    }

    #[must_use]
    pub fn is_payback(&self) -> bool {
        matches!(self, Self::Payback(..))
    }

    #[must_use]
    pub fn is_exact_in(&self) -> bool {
        matches!(self, Self::ExactIn(..))
    }
}
macro_rules! mev_path_to_forge_test_title {
   ($x:literal) => ($x);
    ($x:literal, $($y:ident),+) => (
            $x + mev_path_to_forge_test_title!($($y:ident),+)
    )
}
pub struct MevPathUpdateResult {
    // sorted by highest output transactions
    pub transactions: Vec<Eip1559TransactionRequest>,
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
    pub step: MevPathStep,
}

#[derive(Debug, Clone)]
pub struct PathResult {
    pub ix_data: String,
    pub profit: u128,
    pub is_good: bool,
    pub steps: Vec<StepMeta>,
}

impl MevPath {
    pub async fn new(
        pools: Vec<Pool>,
        pools_locked: &Vec<Arc<RwLock<Pool>>>,
        input_token: &String,
    ) -> Option<Self> {
        let mut re = pools_locked.clone();
        re.reverse();
        let path = re
            .into_iter()
            .map(|p| MevPathStep::ExactOut(p, StepInput::default(), StepOutput::default()))
            .collect::<Vec<MevPathStep>>();

        let mut re = pools.clone();
        re.reverse();
        if let Ok(path) = Self::process_path(path, input_token).await {
            Some(Self {
                input_token: input_token.clone(),
                path,
                pools: re,
            })
        } else {
            None
        }
    }

    fn print_balance(balance: &HashMap<String, HashMap<String, I256>>) {
        let mut fin = "".to_string();
        for (pl, bal) in balance {
            fin += &("\n".to_string() + pl);
            for (token, b) in bal {
                fin += &("\n\t-> ".to_string() + token + " == " + &b.to_string())
            }
        }
        trace!("{}\n", fin);
    }



    async fn chained_out_path(&self, mut path: Vec<MevPathStep>) -> anyhow::Result<PathResult> {
        // binary search for optimal input
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 6.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let decimals = crate::decimals(self.input_token.clone());
        let mut instructions = vec![];
        let mut steps_meta = vec![];

        let contract_address = "<contract_address>".to_string();
        'binary_search: for i in 0..8 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = I256::from(i_atomic as u128);

            let mut instruction = Vec::with_capacity(path.len());
            let mut steps_taken = Vec::with_capacity(path.len());
            let mut sender = "self".to_string();

            // tries to complete steps
            'inner: for (index, step) in path.iter().enumerate() {
                let pool = step.get_pool().await;

                match &step {
                    MevPathStep::ExactIn(_, input, out) | MevPathStep::ExactOut(_, input, out) => {
                        let asset_reciever = out.target.clone();

                        let (asset_token, debt_token) = if pool.x_to_y {
                            (pool.y_address.clone(), pool.x_address.clone())
                        } else {
                            (pool.x_address.clone(), pool.y_address.clone())
                        };
                        if index == 0 && asset_token != self.input_token {
                            return Err(anyhow::Error::msg("Invalid Path"));
                        }

                        trace!(
                            "Recipient: {} Asset_Token: {} Debt_Token: {} ",
                            asset_reciever,
                            asset_token,
                            debt_token
                        );

                        let calculator = pool.provider.build_calculator();
                        // used later to build step instruction
                        let mut d: I256 = I256::from(0);
                        let mut a: I256 = I256::from(0);

                        let dt = debt_token.clone();

                        if asset < I256::from(0) {
                            asset = -asset;
                        }
                        // calculate output for current step
                        let as_uint = asset.into_raw();
                        if let Ok(in_) = calculator.calculate_in(as_uint, &pool) {
                            let debt = I256::from_raw(in_);
                            if debt == I256::zero() {
                                right = mid;
                                mid = (left + right) / 2.0;
                                continue 'binary_search;
                            }
                            a = asset;
                            d = debt;
                            trace!("Type AssetIsDebtedToOther > Asset {} Debt {}", a, d);

                            if pool.provider.id() == LiquidityProviderId::BalancerWeighted
                                && index + 1 != path.len()
                            {
                                trace!("Unproccessable AssetIsDebtedToOther step");
                                return Err(anyhow::Error::msg(
                                    "Unproccessable AssetIsDebtedToOther Step",
                                ));
                            } else {
                                // update asset reciever balance
                                asset = debt
                            }

                            steps_taken.push(StepMeta {
                                step_id: "AssetIsDebtedToOther".to_string(),
                                asset: a,
                                debt: d,
                                asset_token: asset_token.clone(),
                                debt_token: debt_token.clone(),
                                step: step.clone(),
                            });
                        } else {
                            right = mid;
                            mid = (left + right) / 2.0;
                            continue 'binary_search;
                        }

                        match pool.provider.id() {
                            LiquidityProviderId::UniswapV2
                            | LiquidityProviderId::SushiSwap
                            | LiquidityProviderId::Solidly
                            | LiquidityProviderId::Pancakeswap
                            | LiquidityProviderId::CroSwap
                            | LiquidityProviderId::ShibaSwap
                            | LiquidityProviderId::SaitaSwap
                            | LiquidityProviderId::ConvergenceSwap => {
                                // update with reserves
                                let (function, pay_to, token) = if sender == asset_reciever {
                                    (
                                        UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER.to_string(),
                                        "".to_string(),
                                        asset_token[2..].to_string(),
                                    )
                                } else if asset_reciever != contract_address {
                                    (
                                        UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS.to_string(),
                                        asset_reciever[2..].to_string(),
                                        asset_token[2..].to_string(),
                                    )
                                } else {
                                    (
                                        UNISWAP_V2_EXACT_OUT_PAY_TO_SELF.to_string(),
                                        "".to_string(),
                                        "".to_string(),
                                    )
                                };
                                let packed_asset = Self::encode_packed(a);
                                let packed_debt = Self::encode_packed(d);
                                instruction.push(
                                    function
                                        + if pool.x_to_y { "01" } else { "00" }
                                        + &token
                                        + &pool.address[2..]
                                        + &pay_to
                                        + &(packed_asset.len() as u8).encode_hex()[64..]
                                        + &packed_asset,
                                )
                            }
                            LiquidityProviderId::UniswapV3 => {
                                // update with ...
                                let (function, pay_to) = if sender == asset_reciever {
                                    (
                                        UNISWAP_V3_EXACT_OUT_PAY_TO_SENDER.to_string(),
                                        "".to_string(),
                                    )
                                } else if asset_reciever != contract_address {
                                    (
                                        UNISWAP_V3_EXACT_OUT_PAY_TO_ADDRESS.to_string(),
                                        asset_reciever[2..].to_string(),
                                    )
                                } else {
                                    (UNISWAP_V3_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                let packed_asset = Self::encode_packed(a);
                                instruction.push(
                                    function
                                        + if pool.x_to_y { "01" } else { "00" }
                                        + &pool.address[2..]
                                        + &pay_to
                                        + &(packed_asset.len() as u8).encode_hex()[64..]
                                        + &packed_asset,
                                )
                            }
                            LiquidityProviderId::BalancerWeighted => {
                                // update with ...
                                let (function, pay_to) = if sender == asset_reciever {
                                    (BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                } else if asset_reciever != contract_address {
                                    (
                                        BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(),
                                        asset_reciever[2..].to_string(),
                                    )
                                } else {
                                    (BALANCER_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                let packed_asset = Self::encode_packed(a);
                                let packed_debt = Self::encode_packed(d);
                                let meta = match &pool.provider {
                                    LiquidityProviders::BalancerWeighted(meta) => meta,
                                    _ => panic!(),
                                };
                                instruction.push(
                                    function
                                        + &meta.id[2..]
                                        + &debt_token[2..]
                                        + &asset_token[2..]
                                        + &(packed_asset.len() as u8).encode_hex()[64..]
                                        + &packed_asset
                                        + &(packed_debt.len() as u8).encode_hex()[64..]
                                        + &packed_debt,
                                )
                            }
                        }
                        sender = pool.address;
                    }

                    MevPathStep::Payback(_, input, is_x) => {
                        let token = if *is_x {
                            pool.x_address.clone()
                        } else {
                            pool.y_address.clone()
                        };

                        // make sure all other steps are done before paying back
                        trace!("Amount For Payment: {}", asset);
                        steps_taken.push(StepMeta {
                            step_id: "PaybackSender".to_string(),
                            asset: asset,
                            debt: asset,
                            asset_token: token.clone(),
                            debt_token: token.clone(),
                            step: step.clone(),
                        });
                        let packed_amount = Self::encode_packed(asset);
                        instruction.push(
                            PAY_SENDER.to_string()
                                + &token[2..]
                                + &(packed_amount.len() as u8).encode_hex()[64..]
                                + &packed_amount,
                        );
                    }
                }
            }
            instructions.push(instruction);
            steps_meta.push(steps_taken);
            let mut final_balance = sub_i256(I256::from(i_atomic as u128), asset);
            // info!("profit {}, iatomic {} ", final_balance.as_i128() as f64 / 10_f64.powf(18.0), i_atomic);

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

        //                    info!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));
        //
        //                    info!("Size: {} Profit: {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0));
        //                    for step in &steps_meta[best_route_index] {
        //                        info!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
        //                    }
        //                    info!("\n\n\n");
        if best_route_profit > I256::from(0) {
            //            debug!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));
            //
            //            debug!("Size: {} Profit: {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0));
            //            for step in &steps_meta[best_route_index] {
            //                debug!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
            //            }
            //            debug!("\n\n\n");

            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(PathResult {
                ix_data: "".to_string(),
                profit: 0,
                is_good: false,
                steps: vec![],
            })
        }
    }

    fn chained_out_path_sync(&self, mut path: Vec<Pool>) -> anyhow::Result<PathResult> {
        // binary search for optimal input
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = 6.0;

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let decimals = crate::decimals(self.input_token.clone());
        let mut instructions = vec![];
        let mut steps_meta = vec![];

        let contract_address = "<contract_address>".to_string();
        'binary_search: for i in 0..8 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = I256::from(i_atomic as u128);

            let mut instruction = Vec::with_capacity(path.len());
            let mut steps_taken = Vec::with_capacity(path.len());
            let mut sender = "self".to_string();

            // tries to complete steps
            'inner: for (index, pool) in path.iter().enumerate() {

                if index == path.len() - 1 {
                    let token = self.input_token.clone();

                    // make sure all other steps are done before paying back
                        trace!("Amount For Payment: {}", asset);
                    steps_taken.push(StepMeta {
                        step_id: "PaybackSender".to_string(),
                        asset: asset,
                        debt: asset,
                        asset_token: token.clone(),
                        debt_token: token.clone(),
                        step: self.path.get(index).unwrap().clone(),
                    });
                    let packed_amount = Self::encode_packed(asset);
                    instruction.push(
                            PAY_SENDER.to_string()
                            + &token[2..]
                            + &(packed_amount.len() as u8).encode_hex()[64..]
                            + &packed_amount,
                        );
                } else {
                        let asset_reciever = if index == 0 {
                            contract_address.clone()
                        } else {
                            sender.clone()
                        };

                        let (asset_token, debt_token) = if pool.x_to_y {
                            (pool.y_address.clone(), pool.x_address.clone())
                        } else {
                            (pool.x_address.clone(), pool.y_address.clone())
                        };
                        if index == 0 && asset_token != self.input_token {
                            return Err(anyhow::Error::msg("Invalid Path"));
                        }

                        trace!(
                                "Recipient: {} Asset_Token: {} Debt_Token: {} ",
                            asset_reciever,
                            asset_token,
                            debt_token
                        );

                        let calculator = pool.provider.build_calculator();
                        // used later to build step instruction
                        let mut d: I256 = I256::from(0);
                        let mut a: I256 = I256::from(0);

                        let dt = debt_token.clone();

                        if asset < I256::from(0) {
                            asset = -asset;
                        }
                        // calculate output for current step
                        let as_uint = asset.into_raw();
                        if let Ok(in_) = calculator.calculate_in(as_uint, &pool) {
                            let debt = I256::from_raw(in_);
                            if debt == I256::zero() {
                                right = mid;
                                mid = (left + right) / 2.0;
                                continue 'binary_search;
                            }
                            a = asset;
                            d = debt;
                            trace!("Type AssetIsDebtedToOther > Asset {} Debt {}", a, d);

                            if pool.provider.id() == LiquidityProviderId::BalancerWeighted
                               && index + 1 != path.len()
                            {
                                trace!("Unproccessable AssetIsDebtedToOther step");
                                return Err(anyhow::Error::msg(
                                        "Unproccessable AssetIsDebtedToOther Step",
                                ));
                            } else {
                                // update asset reciever balance
                                asset = debt
                            }

                            steps_taken.push(StepMeta {
                                step_id: "AssetIsDebtedToOther".to_string(),
                                asset: a,
                                debt: d,
                                asset_token: asset_token.clone(),
                                debt_token: debt_token.clone(),
                                step: self.path.get(index).unwrap().clone(),
                            });
                        } else {
                            right = mid;
                            mid = (left + right) / 2.0;
                            continue 'binary_search;
                        }

                        match pool.provider.id() {
                            LiquidityProviderId::UniswapV2
                            | LiquidityProviderId::SushiSwap
                            | LiquidityProviderId::Solidly
                            | LiquidityProviderId::Pancakeswap
                            | LiquidityProviderId::CroSwap
                            | LiquidityProviderId::ShibaSwap
                            | LiquidityProviderId::SaitaSwap
                            | LiquidityProviderId::ConvergenceSwap => {
                                // update with reserves
                                let (function, pay_to, token) = if sender == asset_reciever {
                                    (
                                            UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER.to_string(),
                                        "".to_string(),
                                        asset_token[2..].to_string(),
                                    )
                                } else if asset_reciever != contract_address {
                                    (
                                            UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS.to_string(),
                                        asset_reciever[2..].to_string(),
                                        asset_token[2..].to_string(),
                                    )
                                } else {
                                    (
                                            UNISWAP_V2_EXACT_OUT_PAY_TO_SELF.to_string(),
                                        "".to_string(),
                                        "".to_string(),
                                    )
                                };
                                let packed_asset = Self::encode_packed(a);
                                let packed_debt = Self::encode_packed(d);
                                instruction.push(
                                        function
                                        + if pool.x_to_y { "01" } else { "00" }
                                        + &token
                                        + &pool.address[2..]
                                        + &pay_to
                                        + &(packed_asset.len() as u8).encode_hex()[64..]
                                        + &packed_asset,
                                )
                            }
                            LiquidityProviderId::UniswapV3 => {
                                // update with ...
                                let (function, pay_to) = if sender == asset_reciever {
                                    (
                                            UNISWAP_V3_EXACT_OUT_PAY_TO_SENDER.to_string(),
                                        "".to_string(),
                                    )
                                } else if asset_reciever != contract_address {
                                    (
                                            UNISWAP_V3_EXACT_OUT_PAY_TO_ADDRESS.to_string(),
                                        asset_reciever[2..].to_string(),
                                    )
                                } else {
                                    (UNISWAP_V3_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                let packed_asset = Self::encode_packed(a);
                                instruction.push(
                                        function
                                        + if pool.x_to_y { "01" } else { "00" }
                                        + &pool.address[2..]
                                        + &pay_to
                                        + &(packed_asset.len() as u8).encode_hex()[64..]
                                        + &packed_asset,
                                )
                            }
                            LiquidityProviderId::BalancerWeighted => {
                                // update with ...
                                let (function, pay_to) = if sender == asset_reciever {
                                    (BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                } else if asset_reciever != contract_address {
                                    (
                                            BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(),
                                        asset_reciever[2..].to_string(),
                                    )
                                } else {
                                    (BALANCER_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                let packed_asset = Self::encode_packed(a);
                                let packed_debt = Self::encode_packed(d);
                                let meta = match &pool.provider {
                                    LiquidityProviders::BalancerWeighted(meta) => meta,
                                    _ => panic!(),
                                };
                                instruction.push(
                                        function
                                        + &meta.id[2..]
                                        + &debt_token[2..]
                                        + &asset_token[2..]
                                        + &(packed_asset.len() as u8).encode_hex()[64..]
                                        + &packed_asset
                                        + &(packed_debt.len() as u8).encode_hex()[64..]
                                        + &packed_debt,
                                )
                            }
                        }
                        sender = pool.address.clone();


                    }
            }
            instructions.push(instruction);
            steps_meta.push(steps_taken);
            let mut final_balance = sub_i256(I256::from(i_atomic as u128), asset);
            // info!("profit {}, iatomic {} ", final_balance.as_i128() as f64 / 10_f64.powf(18.0), i_atomic);

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

        //                    info!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));
        //
        //                    info!("Size: {} Profit: {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0));
        //                    for step in &steps_meta[best_route_index] {
        //                        info!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
        //                    }
        //                    info!("\n\n\n");
        if best_route_profit > I256::from(0) {
            //            debug!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));
            //
            //            debug!("Size: {} Profit: {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0));
            //            for step in &steps_meta[best_route_index] {
            //                debug!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
            //            }
            //            debug!("\n\n\n");

            let mut final_data = instructions[best_route_index].join("");

            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),
            })
        } else {
            Ok(PathResult {
                ix_data: "".to_string(),
                profit: 0,
                is_good: false,
                steps: vec![],
            })
        }
    }

    async fn path_to_solidity_test(path: &Vec<MevPathStep>, instructions: &Vec<String>) -> String {
        let mut builder = "".to_string();
        let mut title = "function test".to_string();

        let mut final_data = "".to_string();
        for ix in instructions.clone() {
            final_data += &ix;
        }
        for (i, step) in path
            .iter()
            .enumerate()
            .collect::<Vec<(usize, &MevPathStep)>>()
            .into_iter()
        {
            let ix = &instructions[i];
            let function = ix[0..8].to_string();
            match step {
                MevPathStep::ExactIn(pool, _, _) => {
                    title += &(format!("_{:?}", pool.read().await.provider.id())
                        + "ExactIn"
                        + &Self::function_type(function));
                }
                MevPathStep::ExactOut(pool, _, _) => {
                    title += &(format!("_{:?}", pool.read().await.provider.id())
                        + "ExactOut"
                        + &Self::function_type(function));
                }
                MevPathStep::Payback(_, _, _) => {
                    title += &format!("_{}{}", "Payback", &Self::function_type(function));
                }
            }
        }
        builder += &(title + "() public {\n");
        builder += &format!("\n\tbytes memory data = hex\"{}\";", final_data);
        builder += &format!("\n\t(bool success, bytes memory res) = address(agg).call(data);");
        builder += &format!("\n\trequire(success);\n}}");
        builder
    }

    fn function_type(function: String) -> String {
        let res = match function.as_str() {
            "00002000" | "000000d0" | "0000000e" | "000000cd" | "00000080" => "PayToSender",
            "00000600" | "000000fc" | "0e000000" | "00000082" => "PayToSelf",
            "000000c9" | "00000091" | "000000e5" | "00000059" | "00000081" => "PayToAddress",
            _ => "",
        };
        return res.to_string();
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

    pub fn get_transaction_sync(&self, pools_path: Vec<Pool>) -> Option<(Eip1559TransactionRequest, PathResult)> {
        let is_good = self.chained_out_path_sync(pools_path);
        match &is_good {
            Ok(data) => {
                if !data.is_good {
                    return None;
                } else {
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

    pub async fn get_transaction(&self) -> Option<(Eip1559TransactionRequest, PathResult)> {
        let is_good = self.chained_out_path(self.path.clone()).await;
        match &is_good {
            Ok(data) => {
                if !data.is_good {
                    return None;
                } else {
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

    pub fn get_backrun_for_update(
        &self,
        pending_tx: Transaction,
        updated_pool: Pool,
        gas_lookup: &HashMap<String, U256>,
        block_number: u64,
    ) -> Option<Backrun> {
        None
    }

    pub async fn is_valid(&self) -> bool {
        if self.path.len() <= 0 {
            return false;
        }

        let is_good = self.chained_out_path(self.path.clone()).await;
        match &is_good {
            Ok(_) => (true),
            Err(e) => {
                // println!("{:?}", e);
                false
            }
        }
    }

    pub async fn process_path(
        mut path: Vec<MevPathStep>,
        input_token: &String,
    ) -> anyhow::Result<Vec<MevPathStep>> {
        if path.len() < MINIMUM_PATH_LENGTH {
            // return here
        }

        let first_step = path.first().unwrap().clone();
        let mut step_stack: Vec<MevPathStep> = vec![];

        let mut debt: Vec<(MevPathStep, String)> = vec![];
        let mut asset = vec![];
        let mut in_token = input_token.clone();
        asset.push((first_step, in_token));
        let mut balance: HashMap<String, HashMap<String, i8>> = HashMap::new();
        let mut balance1: HashMap<Pool, HashMap<String, bool>> = HashMap::new();
        let contract_address = "<contract_address>".to_string();
        let past_steps: Vec<MevPathStep> = vec![];
        let pools = futures::future::join_all(path.iter().map(|s| async { s.get_pool().await }))
            .await
            .into_iter()
            .collect::<Vec<Pool>>();
        for pool in &pools {
            let mut values = HashMap::new();
            values.insert(pool.x_address.clone(), false);
            values.insert(pool.y_address.clone(), false);
            balance1.insert(pool.clone(), values);
        }

        let path_copy = pools.clone();
        for (index, pool) in pools.iter().enumerate() {
            let (asset_token, debt_token) = if pool.x_to_y {
                (pool.y_address.clone(), pool.x_address.clone())
            } else {
                (pool.x_address.clone(), pool.y_address.clone())
            };

            let mut step_out = StepOutput {
                target: contract_address.clone(),
            };

            let pool_state = balance1.get(&pool).unwrap();
            // the two cases
            // 1. debt_token is input token
            //     -> suports callback payment
            //          register debt and start
            //     -> supports pre payment
            //          pay
            // 2. asset_token is input token
            //     -> suports callback payment
            //          regiester debt and start
            //     -> supports pre payment
            //          filtered later
            if debt_token == *input_token && pool.supports_pre_payment() {
                balance1
                    .get_mut(&pool)
                    .unwrap()
                    .insert(debt_token.clone(), true);
            }
            if asset_token == *input_token {
                balance1
                    .get_mut(&pool)
                    .unwrap()
                    .insert(asset_token.clone(), true);
            } else if let Some(pay_to_step) = pools.iter().find(|p| {
                if p.x_to_y {
                    p.x_address == asset_token && p.supports_callback_payment()
                } else {
                    p.y_address == asset_token && p.supports_callback_payment()
                }
            }) {
                step_out = StepOutput {
                    target: pay_to_step.address.clone(),
                };
                balance1
                    .get_mut(&pay_to_step)
                    .unwrap()
                    .insert(asset_token.clone(), true);
                balance1
                    .get_mut(&pool)
                    .unwrap()
                    .insert(asset_token.clone(), true);
            } else {
                // or pay if there is support for pre payment
                if let Some(pay_to_step) = path_copy.iter().find(|p| {
                    if p.x_to_y {
                        p.x_address == asset_token && p.supports_pre_payment()
                    } else {
                        p.y_address == asset_token && p.supports_pre_payment()
                    }
                }) {
                    step_out = StepOutput {
                        target: pay_to_step.address.clone(),
                    };
                    balance1
                        .get_mut(&pay_to_step)
                        .unwrap()
                        .insert(asset_token.clone(), true);
                    balance1
                        .get_mut(&pool)
                        .unwrap()
                        .insert(asset_token.clone(), true);
                } else {
                    if let Some(pay_to_step) = path_copy.iter().find(|p| {
                        if p.x_to_y {
                            p.x_address == *input_token
                        } else {
                            p.y_address == *input_token
                        }
                    }) {
                        step_out = StepOutput {
                            target: contract_address.clone(),
                        };
                        balance1
                            .get_mut(&pay_to_step)
                            .unwrap()
                            .insert(asset_token.clone(), true);
                        balance1
                            .get_mut(&pool)
                            .unwrap()
                            .insert(asset_token.clone(), true);
                    } else {
                        if let Some(pay_to_step) = path_copy.iter().find(|p| {
                            if p.x_to_y {
                                p.x_address == asset_token
                            } else {
                                p.y_address == asset_token
                            }
                        }) {
                            step_out = StepOutput {
                                target: contract_address.clone(),
                            };
                        } else {
                            return Err(anyhow::Error::msg("Invalid path"));
                        }
                    }
                }
            }
            path[index].update_output(&step_out);

            step_stack.push(path[index].clone());
        }
        for (index, pool) in pools.iter().enumerate() {
            let (asset_token, debt_token) = if pool.x_to_y {
                (pool.y_address.clone(), pool.x_address.clone())
            } else {
                (pool.x_address.clone(), pool.y_address.clone())
            };

            let pool_arc = path[index].get_pool_arc();
            let pool_state = balance1.get(&pool).unwrap();
            if !pool_state.get(&debt_token).unwrap() && pool.supports_callback_payment() {
                step_stack.push(MevPathStep::Payback(
                    pool_arc,
                    StepInput::default(),
                    debt_token == pool.x_address,
                ));
            }
        }
        // if step_stack.len() <= 4 {
        //
        //     for step in &step_stack {
        //         println!("{} -> {}", step, step.get_output());
        //     }
        //     println!("\n\n\nDone path\n\n\n");
        //
        // }
        // for step in &step_stack {
        //     println!("{} -> {}", step, step.get_output());
        // }
        // println!("\n\n\nDone path\n\n\n");

        Ok(step_stack)
    }
}

// keep this logic seprate
struct TransactionBuilder {}
