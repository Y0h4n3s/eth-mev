#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::str::FromStr;
use ethers::types::{Eip1559TransactionRequest, H160, H256, U128, U64, Address, Transaction};
use garb_sync_eth::{
    uniswap_v2::UniswapV2Metadata, uniswap_v3::UniswapV3Metadata, LiquidityProviderId,
    LiquidityProviders, Pool, PoolInfo, UniswapV3Calculator,
};
// helper trait to filter solutions of interest
use crate::MAX_SIZE;
use ethers::abi::{AbiEncode, ParamType, StateMutability, Token};
use itertools::Itertools;
use ethers::types::{U256, I256};
use ethers::types::transaction::eip2930::AccessList;
use tracing::{debug, warn, trace, error, info};
use crate::backrun::Backrun;

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
        _ => "".to_string()
    }
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct MevPath {
    pub path: Vec<MevPathStep>,
    pub pools: Vec<Pool>,
    pub input_token: String,

}


#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct StepInput {
    pub function_hash: String,
    pub pay_to: String,
    pub amount: u128,
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct StepOutput {
    target: String,
}

#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum MevPathStep {
    ExactIn(Pool, StepInput, StepOutput),
    ExactOut(Pool, StepInput, StepOutput),
    // pool: the pool to pay back
    // bool: the token we're paying back, true for x false for y
    Payback(Pool, StepInput, bool),
}

impl Display for MevPathStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MevPathStep::ExactIn(pool, _, _) => write!(f, "ExactIn\n{}\n", pool),
            MevPathStep::ExactOut(pool, _, _) => write!(f, "ExactOut\n{}\n", pool),
            MevPathStep::Payback(pool, _, is_x) => write!(f, "Payback, To X: {}\n{}\n", is_x, pool),
        }
    }
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
            MevPathStep::ExactIn(_, _, output)
            | MevPathStep::ExactOut(_, _, output) =>
                {
                    *output = out.clone();
                }
            MevPathStep::Payback(_, _, _) => ()
        }
    }
    pub fn update_pool(&mut self, updated_pool: &Pool) {
        // x_to_y can never change
        match self {
            MevPathStep::ExactIn(pool, _, _)
            | MevPathStep::ExactOut(pool, _, _)
            | MevPathStep::Payback(pool, _, _) => {
                let mut update = updated_pool.clone();
                update.x_to_y = pool.x_to_y;
                *pool = update;
            }
        }
    }

    pub fn contains_pool(&self, pool: &Pool) -> bool {
        match self {
            MevPathStep::ExactIn(p, _, _)
            | MevPathStep::ExactOut(p, _, _)
            | MevPathStep::Payback(p, _, _) => p.address == pool.address,
        }
    }

    pub fn get_pool(&self) -> Pool {
        match self {
            MevPathStep::ExactIn(p, _, _)
            | MevPathStep::ExactOut(p, _, _)
            | MevPathStep::Payback(p, _, _) => p.clone(),
        }
    }

    pub fn get_output(&self) -> String {
        match self {
            MevPathStep::ExactIn(_, _, o)
            | MevPathStep::ExactOut(_, _, o)
            => o.target.clone(),
            MevPathStep::Payback(p, _, _) => ".".to_string()
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
    pub step: MevPathStep
}

#[derive(Debug, Clone)]
pub struct PathResult {
    pub ix_data: String,
    pub profit: u128,
    pub is_good: bool,
    pub steps: Vec<StepMeta>
}

impl MevPath {
    pub fn new(pools: &Vec<Pool>, input_token: &String) -> Option<Self> {
        let mut re = pools.clone();
        re.reverse();
        let path = re.iter().map(|p| MevPathStep::ExactOut(p.clone(), StepInput::default(), StepOutput::default())).collect::<Vec<MevPathStep>>();

        if let Ok(path) = Self::process_path(path, input_token) {
           Some(Self {
               input_token: input_token.clone(),
               path,
               pools: pools.clone(),
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
    fn estimate_gas_cost(path: &Vec<MevPathStep>) -> u64 {
        let mut gas = 0;
        for p in path {
            match p.get_pool().provider.id() {
                LiquidityProviderId::SushiSwap | LiquidityProviderId::UniswapV2  | LiquidityProviderId::Solidly | LiquidityProviderId::Pancakeswap=> gas += 150000,
                LiquidityProviderId::UniswapV3 | LiquidityProviderId::BalancerWeighted => gas += 250000
            }
        }
        gas
    }
    fn chained_out_path(&self, mut path: Vec<MevPathStep>) -> anyhow::Result<PathResult> {

        // binary search for optimal input
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = if !path.first().unwrap().is_exact_in() && path.first().unwrap().get_pool().supports_callback_payment() {
            60.0
        } else {
            MAX_SIZE.clone() / 2.0
        };

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let decimals = crate::decimals(self.input_token.clone());
        let mut instructions = vec![];
        let mut steps_meta = vec![];

        let contract_address = "<contract_address>".to_string();
        'binary_search: for i in 0..13 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut asset = I256::from(i_atomic as u128);

            let mut instruction = Vec::with_capacity(path.len());
            let mut steps_taken = Vec::with_capacity(path.len());
            let mut sender = "self".to_string();

            // tries to complete steps
            'inner: for (index, step) in path.iter().enumerate().collect::<Vec<(usize, &MevPathStep)>>() {

                trace!("_Step: {} {}", index, step);

                match &step {
                    MevPathStep::ExactIn(pool, input, out) | MevPathStep::ExactOut(pool, input, out) => {
                        let asset_reciever = out.target.clone();

                        let (asset_token, debt_token) = if pool.x_to_y {
                            (pool.y_address.clone(), pool.x_address.clone())
                        } else {
                            (pool.x_address.clone(), pool.y_address.clone())
                        };

                        trace!("Recipient: {} Asset_Token: {} Debt_Token: {} ", asset_reciever, asset_token, debt_token);

                        let calculator = pool.provider.build_calculator();
                        // used later to build step instruction
                        let mut d: I256 = I256::from(0);
                        let mut a: I256 = I256::from(0);

                        let dt = debt_token.clone();

                        if asset < I256::from(0) {
                            asset = -asset;
                        }
                        // calculate output for current step
                        let as_uint = U256::from_dec_str(&asset.to_string());
                        if as_uint.is_err() {
                            trace!("Casting Error: Cast To Uint {}", asset);
                            return Err(anyhow::Error::msg("Casting Error"));
                        }
                        if let Ok(in_) = calculator.calculate_in(as_uint.unwrap(), pool) {
                            let debt = I256::from_dec_str(&in_.to_string()).unwrap();
                            if debt == I256::zero() {
                                right = mid;
                                mid = (left + right) / 2.0;
                                continue 'binary_search;
                            }
                            a = asset;
                            d = debt;
                            trace!("Type AssetIsDebtedToOther > Asset {} Debt {}", a, d);

                            if !step.get_pool().supports_callback_payment() {
                                trace!("Unproccessable AssetIsDebtedToOther step");
                                return Err(anyhow::Error::msg("Unproccessable AssetIsDebtedToOther Step"));
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
                            LiquidityProviderId::UniswapV2 | LiquidityProviderId::SushiSwap | LiquidityProviderId::Solidly | LiquidityProviderId::Pancakeswap => {
                                // update with reserves
                                let (function, pay_to, token) = if sender == asset_reciever {
                                    (UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string(), asset_token[2..].to_string())
                                } else if asset_reciever != contract_address {
                                    (UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string(), asset_token[2..].to_string())
                                } else {
                                    (UNISWAP_V2_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string(), "".to_string())
                                };
                                let packed_asset = Self::encode_packed(a);
                                let packed_debt = Self::encode_packed(d);
                                instruction.push(
                                        function +
                                        if pool.x_to_y { "01" } else { "00" } +
                                        &token +
                                        &pool.address[2..] +
                                        &pay_to +
                                        &(packed_asset.len() as u8).encode_hex()[64..] +
                                        &packed_asset

                                        )
                            }
                            LiquidityProviderId::UniswapV3 => {
                                // update with ...
                                let (function, pay_to) = if sender == asset_reciever {
                                    (UNISWAP_V3_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                } else if asset_reciever != contract_address {
                                    (UNISWAP_V3_EXACT_OUT_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string())
                                } else {
                                    (UNISWAP_V3_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                let packed_asset = Self::encode_packed(a);
                                instruction.push(
                                        function +
                                        if pool.x_to_y { "01" } else { "00" } +
                                        &pool.address[2..] +
                                        &pay_to +
                                        &(packed_asset.len() as u8).encode_hex()[64..] +
                                        &packed_asset
                                        )
                            }
                            LiquidityProviderId::BalancerWeighted => {
                                // update with ...
                                let (function, pay_to) = if sender == asset_reciever {
                                    (BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                } else if asset_reciever != contract_address {
                                    (BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(), asset_reciever[2..].to_string())
                                } else {
                                    (BALANCER_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                let packed_asset = Self::encode_packed(a);
                                let packed_debt = Self::encode_packed(d);
                                let meta = match &pool.provider {
                                    LiquidityProviders::BalancerWeighted(meta) => meta,
                                    _ => panic!()
                                };
                                instruction.push(
                                        function +
                                        &meta.id[2..] +
                                        &debt_token[2..] +
                                        &asset_token[2..] +
                                        &(packed_asset.len() as u8).encode_hex()[64..] +
                                        &packed_asset +
                                        &(packed_debt.len() as u8).encode_hex()[64..] +
                                        &packed_debt
                                        )
                            }

                        }


                    }

                    MevPathStep::Payback(pool, input, is_x) => {
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
                        instruction.push(PAY_SENDER.to_string() + &token[2..]  + &(packed_amount.len() as u8).encode_hex()[64..] + &packed_amount);
                    }
                }

                sender = step.get_pool().address;

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
                }
                else if final_balance >= best_route_profit {
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
    
        if best_route_profit > I256::from(0) {
            debug!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));

            debug!("Size: {} Profit: {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0));
            for step in &steps_meta[best_route_index] {
                debug!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
            }
            debug!("\n\n\n");

            let mut final_data = "".to_string();
            for ix in instructions[best_route_index].clone() {
                final_data += &ix;
            }
            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),

            })
        } else if best_route_profit == I256::from(0) {
            Err(anyhow::Error::msg("Inv Path"))
        } else {
            Ok(PathResult {
                ix_data: "".to_string(),
                profit: 0,
                is_good: false,
                steps: vec![]
            })
        }
    }
        fn validate_path(&self, mut path: Vec<MevPathStep>) -> anyhow::Result<PathResult> {
        // binary search for optimal input
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = if !path.first().unwrap().is_exact_in() && path.first().unwrap().get_pool().supports_callback_payment() {
           60.0
        } else {
            MAX_SIZE.clone() / 2.0
        };

        let mut left = 0.0;
        let mut right = mid * 2.0;
        let decimals = crate::decimals(self.input_token.clone());
        let mut instructions = vec![];
        let mut steps_meta = vec![];


        let pre_pay = !path.first().unwrap().get_pool().supports_callback_payment();

        //wether the current size has been used by the path
        let mut size_used = false;
        let non_payback_steps = path.iter()
            .enumerate()
            .filter_map(|(index, p)| {
                if p.is_payback() {
                    None
                } else {
                    Some(index)
                }
            }).collect::<Vec<usize>>();
        trace!("\n\n\n");

        // initially fill balances with 0 and contract wallet balance with current size
        let mut balance: HashMap<String, HashMap<String, I256>> = HashMap::new();
        let contract_address = "<contract_address>".to_string();

        for step in &path {
            let pool = step.get_pool();

            for step in &path {
                let pool1 = step.get_pool();
                if balance.get(&pool.address).is_some() {
                    balance
                        .get_mut(&pool.address)
                        .unwrap()
                        .insert(pool1.x_address, I256::from(0));
                    balance
                        .get_mut(&pool.address)
                        .unwrap()
                        .insert(pool1.y_address, I256::from(0));
                } else {
                    let mut map = HashMap::new();
                    map.insert(pool1.x_address, I256::from(0));
                    map.insert(pool1.y_address, I256::from(0));
                    balance.insert(pool.address.clone(), map);
                }
            }
            if balance.get(&contract_address).is_some() {
                balance
                    .get_mut(&contract_address)
                    .unwrap()
                    .insert(pool.x_address, I256::from(0));
                balance
                    .get_mut(&contract_address)
                    .unwrap()
                    .insert(pool.y_address, I256::from(0));
            } else {
                let mut map = HashMap::new();
                map.insert(pool.x_address, I256::from(0));
                map.insert(pool.y_address, I256::from(0));
                balance.insert(contract_address.clone(), map);
            }
        }
        'binary_search: for i in 0..13 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;

            let mut balance = balance.clone();

            let mut instruction = Vec::with_capacity(path.len());
            let mut steps_taken = Vec::with_capacity(path.len());
            let mut steps_done: Vec<usize> = vec![];

            'stepper: for j in 0..path.len() {
                if steps_done.len() >= path.len() {
                    break;
                }
                let mut sender = "Contract_owner".to_string();
                let mut sender_stack = vec![];
                sender_stack.push(sender.clone());
                // tries to complete steps
                'inner: for (index, step) in path.iter().enumerate().collect::<Vec<(usize, &MevPathStep)>>() {
                    if steps_done.contains(&index) {
                        if step.is_payback() {
                            sender = sender_stack.pop().unwrap()
                        } else {
                            sender = step.get_pool().address.clone();
                            sender_stack.push(sender.clone())
                        }
                        continue 'inner;
                    }
                    trace!("_Step: {} {} {}", index, j, step);

                    match &step {
                        MevPathStep::ExactIn(pool, input, out) | MevPathStep::ExactOut(pool, input, out) => {
                            let asset_reciever = out.target.clone();

                            let (asset_token, debt_token) = if pool.x_to_y {
                                (pool.y_address.clone(), pool.x_address.clone())
                            } else {
                                (pool.x_address.clone(), pool.y_address.clone())
                            };
                            trace!("Recipient: {} Asset_Token: {} Debt_Token: {} ", asset_reciever, asset_token, debt_token);

                            let calculator = pool.provider.build_calculator();
                            // used later to build step instruction
                            let mut d: I256 = I256::from(0);
                            let mut a: I256 = I256::from(0);

                            let dt = debt_token.clone();
                            if step.get_pool().supports_pre_payment() && *balance.get(&pool.address).unwrap().get(&debt_token).unwrap() > I256::from(0) {
                                let mut debt = *balance.get(&pool.address).unwrap().get(&debt_token).unwrap();
                                if debt < I256::from(0) {
                                    debt = -debt;
                                }

                                // add the debt to current pool
                                let as_uint = U256::from_dec_str(&debt.to_string());
                                if as_uint.is_err() {
                                    trace!("Casting Error: Cast To Uint {}", debt);

                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                if let Ok(out_) = calculator.calculate_out(as_uint.unwrap(), pool) {
                                    let asset = I256::from_dec_str(&out_.to_string()).unwrap();
                                    if asset == I256::zero() {
                                        right = mid;
                                        mid = (left + right) / 2.0;
                                        continue 'binary_search;
                                    }
                                    a = asset;
                                    d = debt;
                                    steps_taken.push(StepMeta {
                                        step_id: "PrePaid".to_string(),
                                        asset: a,
                                        debt: d,
                                        asset_token: asset_token.clone(),
                                        debt_token: debt_token.clone(),
                                        step: step.clone(),
                                    });
                                    trace!("Type PrePaid > Asset {} Debt {}", a, d);

                                    let bal = *balance
                                        .get(&pool.address)
                                        .unwrap()
                                        .get(&debt_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&pool.address)
                                        .unwrap()
                                        .insert(debt_token.clone(), sub_i256(bal, debt));
                                    let bal = *balance
                                        .get(&asset_reciever)
                                        .unwrap()
                                        .get(&asset_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&asset_reciever)
                                        .unwrap()
                                        .insert(asset_token.clone(), bal + (asset));
                                } else {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;

                                }

                            } else if let Some((from, b)) = balance.iter().find(|(p, b)| {
                                if *p == &contract_address {
                                    return false;
                                } else {
                                    let balance = *b.get(&asset_token).unwrap();
                                    let pool = path.iter().find(|pl| &pl.get_pool().address == *p).unwrap().get_pool();
                                    pool.address == asset_reciever && balance < I256::from(0)
                                }
                            }) {
                                // if no callback payment transfer from contract_address if available


                                let mut asset = *b.get(&asset_token).unwrap();

                                if asset < I256::from(0) {
                                    asset = -asset;
                                }
                                // calculate output for current step
                                let as_uint = U256::from_dec_str(&asset.to_string());
                                if as_uint.is_err() {
                                    trace!("Casting Error: Cast To Uint {}", asset);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                if let Ok(in_) = calculator.calculate_in(as_uint.unwrap(), pool) {
                                    let debt = I256::from_dec_str(&in_.to_string()).unwrap();
                                    if debt == I256::zero() {
                                        right = mid;
                                        mid = (left + right) / 2.0;
                                        continue 'binary_search;
                                    }
                                    a = asset;
                                    d = debt;
                                    trace!("Type AssetIsDebtedToOther > Asset {} Debt {}", a, d);

                                    if !step.get_pool().supports_callback_payment() {
                                        let contract_balance = *balance
                                            .get(&contract_address)
                                            .unwrap()
                                            .get(&debt_token)
                                            .unwrap();
                                        if contract_balance <= I256::from(0) {
                                            trace!("Unproccessable AssetIsDebtedToOther step");
                                            return Err(anyhow::Error::msg("Unproccessable AssetIsDebtedToOther Step"));
                                        }

                                        balance
                                            .get_mut(&contract_address)
                                            .unwrap()
                                            .insert(debt_token.clone(), sub_i256(contract_balance, debt));
                                        balance
                                            .get_mut(&pool.address)
                                            .unwrap()
                                            .insert(debt_token.clone(), I256::from(0));
                                    } else {
                                        // update asset reciever balance
                                        let bal = *balance
                                            .get(&pool.address)
                                            .unwrap()
                                            .get(&debt_token)
                                            .unwrap();
                                        balance
                                            .get_mut(&pool.address)
                                            .unwrap()
                                            .insert(debt_token.clone(), sub_i256(bal, debt));
                                    }

                                    let bal = *balance
                                        .get(&asset_reciever)
                                        .unwrap()
                                        .get(&asset_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&asset_reciever)
                                        .unwrap()
                                        .insert(asset_token.clone(), bal + (asset));

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

                            } else if let Some((from, b)) = balance.iter().find(|(p, b)| {
                                if *p == &contract_address {
                                    return *b.get(&debt_token).unwrap() > I256::from(0);
                                } else {
                                    let balance = *b.get(&debt_token).unwrap();
                                    let pool = path.iter().find(|pl| &pl.get_pool().address == *p).unwrap().get_pool();
                                    if pool.x_to_y {
                                        pool.y_address == debt_token && balance > I256::from(0)
                                    } else {
                                        pool.x_address == debt_token && balance > I256::from(0)
                                    }
                                }
                            }) {
                                let mut debt = *b.get(&debt_token).unwrap();
                                if debt < I256::from(0) {
                                    debt = -debt;
                                }

                                // add the debt to current pool
                                let as_uint = U256::from_dec_str(&debt.to_string());
                                if as_uint.is_err() {
                                    trace!("Casting Error: Cast To Uint {}", debt);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }

                                if let Ok(out_) = calculator.calculate_out(as_uint.unwrap(), pool) {
                                    let asset = I256::from_dec_str(&out_.to_string()).unwrap();
                                    if asset == I256::zero() {
                                        right = mid;
                                        mid = (left + right) / 2.0;
                                        continue 'binary_search;
                                    }
                                    a = asset;
                                    d = debt;
                                    trace!("Type DebtIsAssetOnSelf > Asset {} Debt {}", a, d);
                                    steps_taken.push(StepMeta {
                                        step_id: "DebtIsAssetOnSelf".to_string(),
                                        asset: a,
                                        debt: d,
                                        asset_token: asset_token.clone(),
                                        debt_token: debt_token.clone(),
                                        step: step.clone(),
                                    });
                                    let bal = *balance
                                        .get(&pool.address)
                                        .unwrap()
                                        .get(&debt_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&pool.address)
                                        .unwrap()
                                        .insert(debt_token.clone(), sub_i256(bal, debt));
                                    let bal = *balance
                                        .get(&asset_reciever)
                                        .unwrap()
                                        .get(&asset_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&asset_reciever)
                                        .unwrap()
                                        .insert(asset_token.clone(), bal + (asset));
                                } else {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;
                                }

                            } else if debt_token == self.input_token {
                                let debt = I256::from(i_atomic as u128);

                                // add the debt to current pool
                                let as_uint = U256::from_dec_str(&debt.to_string());
                                if as_uint.is_err() {
                                    trace!("Casting Error: Cast To Uint {}", debt);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }

                                if let Ok(out_) = calculator.calculate_out(as_uint.unwrap(), pool) {
                                    let asset = I256::from_dec_str(&out_.to_string()).unwrap();
                                    if asset == I256::zero() {
                                        right = mid;
                                        mid = (left + right) / 2.0;
                                        continue 'binary_search;
                                    }
                                    a = asset;
                                    d = debt;
                                    trace!("Type DebtIsInputToken > Asset {} Debt {}", a, d);
                                    if !step.get_pool().supports_callback_payment() {
                                        balance
                                            .get_mut(&contract_address)
                                            .unwrap()
                                            .insert(debt_token.clone(), -debt);
                                        balance
                                            .get_mut(&pool.address)
                                            .unwrap()
                                            .insert(debt_token.clone(), I256::from(0));
                                    } else {
                                        // update asset reciever balance
                                        let bal = *balance
                                            .get(&pool.address)
                                            .unwrap()
                                            .get(&debt_token)
                                            .unwrap();
                                        balance
                                            .get_mut(&pool.address)
                                            .unwrap()
                                            .insert(debt_token.clone(), sub_i256(bal, debt));
                                    }
                                    steps_taken.push(StepMeta {
                                        step_id: "DebtIsInputToken".to_string(),
                                        asset: a,
                                        debt: d,
                                        asset_token: asset_token.clone(),
                                        debt_token: debt_token.clone(),
                                        step: step.clone(),
                                    });
                                    let bal = *balance
                                        .get(&asset_reciever)
                                        .unwrap()
                                        .get(&asset_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&asset_reciever)
                                        .unwrap()
                                        .insert(asset_token.clone(), bal + (asset));
                                } else {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;
                                }

                            } else if asset_token == self.input_token && step.get_pool().supports_callback_payment() {
                                let asset = I256::from(i_atomic as u128);

                                let as_uint = U256::from_dec_str(&asset.to_string());
                                if as_uint.is_err() {
                                    trace!("Casting Error: Cast To Uint {}", asset);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                if let Ok(in_) = calculator.calculate_in(as_uint.unwrap(), pool) {
                                    let debt = I256::from_dec_str(&in_.to_string()).unwrap();
                                    if debt == I256::zero() {
                                        right = mid;
                                        mid = (left + right) / 2.0;
                                        continue 'binary_search;
                                    }
                                    a = asset;
                                    d = debt;
                                    trace!("Type AssetIsInputToken > Asset {} Debt {}", a, d);
                                    steps_taken.push(StepMeta {
                                        step_id: "AssetIsInputToken".to_string(),
                                        asset: a,
                                        debt: d,
                                        asset_token: asset_token.clone(),
                                        debt_token: debt_token.clone(),
                                        step: step.clone(),
                                    });
                                    // update asset reciever balance
                                    let bal = *balance
                                        .get(&pool.address)
                                        .unwrap()
                                        .get(&debt_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&pool.address)
                                        .unwrap()
                                        .insert(debt_token.clone(), sub_i256(bal, debt));
                                    let bal = *balance
                                        .get(&asset_reciever)
                                        .unwrap()
                                        .get(&asset_token)
                                        .unwrap();
                                    balance
                                        .get_mut(&asset_reciever)
                                        .unwrap()
                                        .insert(asset_token.clone(), bal + (asset));
                                }
                                else {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;
                                }

                            } else if asset_token == self.input_token && index == 0 {
                                trace!("Unproccessable step");
                                return Err(anyhow::Error::msg("Unproccessable Step"));
                            } else {
                                trace!("Can not process step");
                                if step.get_pool().supports_callback_payment() {
                                    continue 'inner;
                                } else {
                                    return Err(anyhow::Error::msg("Unproccessable Step"));
                                }
                            }

                                match pool.provider.id() {
                                    LiquidityProviderId::UniswapV2 | LiquidityProviderId::SushiSwap | LiquidityProviderId::Solidly | LiquidityProviderId::Pancakeswap => {
                                        // update with reserves
                                        let (function, pay_to, token) = if sender == asset_reciever {
                                            (UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string(), asset_token[2..].to_string())
                                        } else if asset_reciever != contract_address {
                                            (UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string(), asset_token[2..].to_string())
                                        } else {
                                            (UNISWAP_V2_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string(), "".to_string())
                                        };
                                        let packed_asset = Self::encode_packed(a);
                                        let packed_debt = Self::encode_packed(d);
                                        instruction.push(
                                            function +
                                                if pool.x_to_y { "01" } else { "00" } +
                                                &token +
                                                &pool.address[2..] +
                                                &pay_to +
                                                &(packed_asset.len() as u8).encode_hex()[64..] +
                                                &packed_asset

                                        )
                                    }
                                    LiquidityProviderId::UniswapV3 => {
                                        // update with ...
                                        let (function, pay_to) = if sender == asset_reciever {
                                            (UNISWAP_V3_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                        } else if asset_reciever != contract_address {
                                            (UNISWAP_V3_EXACT_OUT_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string())
                                        } else {
                                            (UNISWAP_V3_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                        };
                                        let packed_asset = Self::encode_packed(a);
                                        instruction.push(
                                            function +
                                                if pool.x_to_y { "01" } else { "00" } +
                                                &pool.address[2..] +
                                                &pay_to +
                                                &(packed_asset.len() as u8).encode_hex()[64..] +
                                                &packed_asset
                                        )
                                    }
                                    LiquidityProviderId::BalancerWeighted => {
                                        // update with ...
                                        let (function, pay_to) = if sender == asset_reciever {
                                            (BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                        } else if asset_reciever != contract_address {
                                            (BALANCER_EXACT_OUT_PAY_TO_SENDER.to_string(), asset_reciever[2..].to_string())
                                        } else {
                                            (BALANCER_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                        };
                                        let packed_asset = Self::encode_packed(a);
                                        let packed_debt = Self::encode_packed(d);
                                        let meta = match &pool.provider {
                                            LiquidityProviders::BalancerWeighted(meta) => meta,
                                            _ => panic!()
                                        };
                                        instruction.push(
                                            function +
                                                &meta.id[2..] +
                                                &debt_token[2..] +
                                                &asset_token[2..] +
                                                &(packed_asset.len() as u8).encode_hex()[64..] +
                                                &packed_asset +
                                                &(packed_debt.len() as u8).encode_hex()[64..] +
                                                &packed_debt
                                        )
                                    }

                                }



                            // step is complete if we got here
                            steps_done.push(index);
                        }

                        MevPathStep::Payback(pool, input, is_x) => {
                            let token = if *is_x {
                                pool.x_address.clone()
                            } else {
                                pool.y_address.clone()
                            };

                            // make sure all other steps are done before paying back
                            if !non_payback_steps.iter().all(|s| steps_done.contains(s)) {
                                continue 'inner;
                            }
                            let mut amount_to_pay = *balance.get(&contract_address).unwrap().get(&token).unwrap();
                            trace!("Amount For Payment: {}", amount_to_pay);
                            let (function, pay_to) = if sender == pool.address {
                                if *balance.get(&sender).unwrap().get(&token).unwrap() > I256::from(0) {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;
                                } else {
                                    // trace!("enough SENDER: {} {}", amount_to_pay.to_string(), balance.get(&sender).unwrap().get(&token).unwrap());

                                    let bal = *balance
                                        .get(&sender)
                                        .unwrap()
                                        .get(&token)
                                        .unwrap();
                                    trace!("Due Balance: {}", bal);
                                    trace!("Left: {}", sub_i256(amount_to_pay, bal));
                                    balance.get_mut(&contract_address).unwrap().insert(token.clone(), sub_i256(amount_to_pay, bal));

                                    amount_to_pay = bal;
                                    balance
                                        .get_mut(&sender)
                                        .unwrap()
                                        .insert(token.clone(), I256::from(0));
                                }
                                steps_taken.push(StepMeta {
                                    step_id: "PaybackSender".to_string(),
                                    asset: amount_to_pay,
                                    debt: amount_to_pay,
                                    asset_token: token.clone(),
                                    debt_token: token.clone(),
                                    step: step.clone(),
                                });
                                (PAY_SENDER.to_string(), "".to_string())
                            } else {
                                if *balance.get(&pool.address).unwrap().get(&token).unwrap() > I256::from(0) {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;
                                } else {
                                    // trace!("enough: {} {}", amount_to_pay.to_string(), balance.get(&pool.address).unwrap().get(&token).unwrap());

                                    let bal = *balance
                                        .get(&pool.address)
                                        .unwrap()
                                        .get(&token)
                                        .unwrap();
                                    trace!("Due Balance: {}", bal);
                                    trace!("Left: {}", sub_i256(amount_to_pay, bal));

                                    balance.get_mut(&contract_address).unwrap().insert(token.clone(), sub_i256(amount_to_pay, bal));

                                    amount_to_pay = bal;
                                    balance
                                        .get_mut(&pool.address)
                                        .unwrap()
                                        .insert(token.clone(), I256::from(0));
                                }
                                steps_taken.push(StepMeta {
                                    step_id: "Payback_address".to_string(),
                                    asset: amount_to_pay,
                                    debt: amount_to_pay,
                                    asset_token: token.clone(),
                                    debt_token: token.clone(),
                                    step: step.clone(),
                                });
                                (PAY_ADDRESS.to_string(), pool.address.clone()[2..].to_string())
                            };

                            let packed_amount = Self::encode_packed(amount_to_pay);
                            instruction.push(function + &token[2..] + &pay_to + &(packed_amount.len() as u8).encode_hex()[64..] + &packed_amount);
                            steps_done.push(index);
                        }
                    }
                    // Self::print_balance(&balance);
                    if step.is_payback() {
                        sender = sender_stack.pop().unwrap()
                    } else {
                        sender = step.get_pool().address.clone();
                        sender_stack.push(sender.clone())
                    }
                }
            }
            instructions.push(instruction);
            steps_meta.push(steps_taken);
            let asset_balances = balance.iter().map(|(p, b)| b.get(&self.input_token).unwrap()).map(|b| if *b < I256::from(0) { -*b } else { *b }).filter(|b| b != &I256::from(i_atomic as u128)).sorted().collect::<Vec<I256>>();
            if steps_done.len() != path.len() {
                trace!("{} {}", steps_done.len(), path.len());
                return Err(anyhow::Error::msg("Invalid Path: Not All Steps Done in Path"));
            } else {
                let mut final_balance = *balance.get(&contract_address).unwrap().get(&self.input_token).unwrap();
                let profit = sub_i256(final_balance, I256::from(i_atomic as u128));
                // info!("profit {}, iatomic {} ", final_balance.as_i128() as f64 / 10_f64.powf(18.0), i_atomic);
                Self::print_balance(&balance);

                if i == 0 {
                    best_route_profit = final_balance;
                    best_route_size = i_atomic;
                    if best_route_profit > I256::from(0) {
                        left = mid;
                    } else {
                        right = mid;
                    }
                }
                else if final_balance >= best_route_profit {
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
        }
        // debug!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));
        //
        // if steps_meta.len() > 0 {
        //     debug!("Size: {} Profit: {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0));
        //     for step in &steps_meta[best_route_index] {
        //         debug!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
        //     }
        //     debug!("\n\n\n");
        // }

        if best_route_profit > I256::from(0) {
                debug!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));

                debug!("Size: {} Profit: {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0));
                for step in &steps_meta[best_route_index] {
                    debug!("{} -> {}\n Type: {}\nAsset: {} => {}\n Debt: {} => {} ", step.step, step.step.get_output(), step.step_id, step.asset_token, step.asset, step.debt_token, step.debt);
                }
                debug!("\n\n\n");

            let mut final_data = "".to_string();
            for ix in instructions[best_route_index].clone() {
                final_data += &ix;
            }
            Ok(PathResult {
                ix_data: final_data,
                profit: best_route_profit.as_u128(),
                is_good: true,
                steps: steps_meta[best_route_index].clone(),

            })
        } else if best_route_profit == I256::from(0) {
            Err(anyhow::Error::msg("Inv Path"))
        } else {
            Ok(PathResult {
                ix_data: "".to_string(),
                profit: 0,
                is_good: false,
                steps: vec![]
            })
        }
    }

    fn path_to_solidity_test(path: &Vec<MevPathStep>, instructions: &Vec<String>) -> String {
        let mut builder = "".to_string();
        let mut title = "function test".to_string();

        let mut final_data = "".to_string();
        for ix in instructions.clone() {
            final_data += &ix;
        }
        for (i, step) in path.iter().enumerate().collect::<Vec<(usize, &MevPathStep)>>().into_iter() {
            let ix = &instructions[i];
            let function = ix[0..8].to_string();
            match step {
                MevPathStep::ExactIn(pool, _, _) => {
                    title += &(format!("_{:?}", pool.provider.id()) + "ExactIn" + &Self::function_type(function));
                }
                MevPathStep::ExactOut(pool, _, _) => {
                    title += &(format!("_{:?}", pool.provider.id()) + "ExactOut" + &Self::function_type(function));
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
            _ => ""
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
    pub fn update(&mut self, updated_pool: Pool) {
            for mut step in (*self.path).iter_mut() {
                if step.contains_pool(&updated_pool) {
                    step.update_pool(&updated_pool);
                }
            }
    }

    pub fn get_transaction(&self) -> Option<(Eip1559TransactionRequest, PathResult)> {
        let is_good = self.chained_out_path(self.path.clone());
        match &is_good {
            Ok(data) => {
                if !data.is_good {
                    return None;
                } else {
                    let function_name = hash_to_function_name(&data.ix_data[2..10].to_string());
                    trace!("Entry function {}", function_name);
                    let call_function = ethers::abi::Function {
                        name: function_name,
                        inputs: vec![
                            ethers::abi::Param {
                                name: "data".to_string(),
                                kind: ParamType::Bytes,
                                internal_type: None,
                            },
                        ],
                        outputs: vec![],
                        constant: None,
                        state_mutability: StateMutability::View,
                    };


                    let tx_data = ethers::contract::encode_function_data(&call_function, Token::Bytes(ethers::types::Bytes::from_str(&data.ix_data).unwrap().to_vec())).unwrap();


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
                info!("{:?}", e);
                None
            }
        }

    }

    pub fn get_backrun_for_update(&self, pending_tx: Transaction, updated_pool: Pool, gas_lookup: &HashMap<String, U256>, block_number: u64) -> Option<Backrun> {
        let mut mock = self.clone();
        mock.update(updated_pool);
        if let Some((tx, result)) = mock.get_transaction() {
            let gas_cost = gas_lookup.iter().filter_map(|(pl, amount)| {
                if mock.pools.iter().any(|p | &p.address == pl) {

                    Some(amount)
                } else {
                    None
                }
            }).cloned()
                .reduce(|a, b| a + b)
                .unwrap_or(U256::from(400000));

            Some(Backrun {
                tx,
                pending_tx,
                path: mock,
                profit: U256::from(result.profit),
                gas_cost,
                block_number,
                result

            })
        } else {
            None
        }
    }

    pub fn is_valid(&self) -> bool {
        if self.path.len() <= 0 {
            return false;
        }

        let is_good = self.validate_path(self.path.clone());
        match &is_good {
            Ok(_) => (true),
            Err(e) => {
                // println!("{:?}", e);
                false
            }
        }
    }

    pub fn process_path(
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
        for step in &path {
            let pool = step.get_pool();
            let mut values = HashMap::new();
            values.insert(pool.x_address.clone(), false);
            values.insert(pool.y_address.clone(), false);
            balance1.insert(pool.clone(), values);
        }

        let path_copy = path.clone();
        for step in path.iter_mut() {
            let (asset_token, debt_token) = match step {
                MevPathStep::ExactIn(pool, _, _) | MevPathStep::ExactOut(pool, _, _) => {
                    if pool.x_to_y {
                        (pool.y_address.clone(), pool.x_address.clone())
                    } else {
                        (pool.x_address.clone(), pool.y_address.clone())
                    }
                }
                _ => ("".to_string(), "".to_string()),
            };
            let mut step_out =
                StepOutput {
                    target: contract_address.clone()
                };

            let pool = step.get_pool();
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
                balance1.get_mut(&pool).unwrap().insert(debt_token.clone(), true);
            }
            if asset_token == *input_token {
                balance1.get_mut(&pool).unwrap().insert(asset_token.clone(), true);
            } else if let Some(pay_to_step) = step_stack.iter().find(|s| {
                let p = s.get_pool();
                if p.x_to_y {
                    p.x_address == asset_token && p.supports_callback_payment()
                } else {
                    p.y_address == asset_token && p.supports_callback_payment()
                }
            }) {
                step_out = StepOutput {
                    target: pay_to_step.get_pool().address
                };
                balance1.get_mut(&pay_to_step.get_pool()).unwrap().insert(asset_token.clone(), true);
                balance1.get_mut(&pool).unwrap().insert(asset_token.clone(), true);
            } else {
                // or pay if there is support for pre payment
                if let Some(pay_to_step) = path_copy[step_stack.len()..].iter().find(|s| {
                    let p = s.get_pool();
                    if p.x_to_y {
                        p.x_address == asset_token && p.supports_pre_payment()
                    } else {
                        p.y_address == asset_token && p.supports_pre_payment()
                    }
                }) {
                    step_out = StepOutput {
                        target: pay_to_step.get_pool().address
                    };
                    balance1.get_mut(&pay_to_step.get_pool()).unwrap().insert(asset_token.clone(), true);
                    balance1.get_mut(&pool).unwrap().insert(asset_token.clone(), true);
                } else {
                    if let Some(pay_to_step) = path_copy.iter().find(|s| {
                        let p = s.get_pool();
                        if p.x_to_y {
                            p.x_address == *input_token
                        } else {
                            p.y_address == *input_token
                        }
                    }) {
                        step_out = StepOutput {
                            target: contract_address.clone()
                        };
                        balance1.get_mut(&pay_to_step.get_pool()).unwrap().insert(asset_token.clone(), true);
                        balance1.get_mut(&pool).unwrap().insert(asset_token.clone(), true);
                    } else {
                        if let Some(pay_to_step) = path_copy.iter().find(|s| {
                            let p = s.get_pool();
                            if p.x_to_y {
                                p.x_address == asset_token
                            } else {
                                p.y_address == asset_token
                            }
                        }) {
                            step_out = StepOutput {
                                target: contract_address.clone()
                            };
                        } else {
                            return Err(anyhow::Error::msg("Invalid path"));
                        }
                    }
                }
            }
            step.update_output(&step_out);

            step_stack.push(step.clone());
        }
        for step in &path {
            let (asset_token, debt_token) = match step {
                MevPathStep::ExactIn(pool, _, _) | MevPathStep::ExactOut(pool, _, _) => {
                    if pool.x_to_y {
                        (pool.y_address.clone(), pool.x_address.clone())
                    } else {
                        (pool.x_address.clone(), pool.y_address.clone())
                    }
                }
                _ => ("".to_string(), "".to_string()),
            };
            let pool = step.get_pool();
            let pool_state = balance1.get(&pool).unwrap();
            if !pool_state.get(&debt_token).unwrap() && pool.supports_callback_payment() {
                step_stack.push(MevPathStep::Payback(
                    pool.clone(),
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
