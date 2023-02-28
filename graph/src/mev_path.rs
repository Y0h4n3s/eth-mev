#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::{Debug, Display};

use ethers::types::{Eip1559TransactionRequest, U128};
use garb_sync_eth::{
    uniswap_v2::UniswapV2Metadata, uniswap_v3::UniswapV3Metadata, LiquidityProviderId,
    LiquidityProviders, Pool, PoolInfo, UniswapV3Calculator,
};
// helper trait to filter solutions of interest
use crate::MAX_SIZE;
use ethers::abi::AbiEncode;
use itertools::Itertools;
use ethers::types::{U256, I256};
use tracing::{warn, debug, error, info};
const MINIMUM_PATH_LENGTH: usize = 2;
const UNISWAP_V3_EXACT_OUT_PAY_TO_SENDER: &str = "0000004b";
const UNISWAP_V3_EXACT_IN_PAY_TO_SENDER: &str = "000000d0";
const UNISWAP_V3_EXACT_OUT_PAY_TO_SELF: &str = "000000e1";
const UNISWAP_V3_EXACT_IN_PAY_TO_SELF: &str = "000000fc";
const UNISWAP_V3_EXACT_OUT_PAY_TO_ADDRESS: &str = "000000c9";
const UNISWAP_V3_EXACT_IN_PAY_TO_ADDRESS: &str = "00000091";

const UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER: &str = "00000015";
const UNISWAP_V2_EXACT_IN_PAY_TO_SENDER: &str = "000000cd";
const UNISWAP_V2_EXACT_OUT_PAY_TO_SELF: &str = "0000003c";
const UNISWAP_V2_EXACT_IN_PAY_TO_SELF: &str = "00000082";
const UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS: &str = "000000e5";
const UNISWAP_V2_EXACT_IN_PAY_TO_ADDRESS: &str = "00000059";

const PAY_ADDRESS: &str = "00000081";
const PAY_SENDER: &str = "000000ea";


#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct MevPath {
    pub paths: Vec<Vec<MevPathStep>>,
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
        match self {
            MevPathStep::ExactIn(pool, _, _)
            | MevPathStep::ExactOut(pool, _, _)
            | MevPathStep::Payback(pool, _, _) => {
                *pool = updated_pool.clone();
            }
        }
    }

    pub fn contains_pool(&self, pool: &Pool) -> bool {
        match self {
            MevPathStep::ExactIn(p, _, _)
            | MevPathStep::ExactOut(p, _, _)
            | MevPathStep::Payback(p, _, _) => p == pool,
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

impl MevPath {
    pub fn new(pools: &Vec<Pool>, input_token: &String) -> Self {
        let mut re = pools.clone();
        re.reverse();
        let mut expanded = vec![];

        for (index, pls) in [/*pools.clone(),*/ re].iter().enumerate() {
            for i in 0..2_u64.pow(pls.len() as u32) {
                expanded.push(vec![]);
            }
            let input = StepInput::default();
            let output = StepOutput::default();
            for j in 0..pls.len() {
                let mut count = 0;
                let mut to_zero = false;
                let mut first_to_zero = true;
                for i in 0..2_u64.pow(pls.len() as u32) {
                    if count < (2_u64.pow((pls.len() - j) as u32)) / 2 && !to_zero {
                        count += 1;
                        expanded[i as usize + index * 2_u64.pow(pls.len() as u32) as usize].push(MevPathStep::ExactIn(pls[j].clone(), input.clone(), output.clone()));
                    } else {
                        if first_to_zero {
                            to_zero = true;
                            first_to_zero = false;
                        }

                        count -= 1;
                        expanded[i as usize + index * 2_u64.pow(pls.len() as u32) as usize].push(MevPathStep::ExactOut(pls[j].clone(), input.clone(), output.clone()));
                        if count == 0 {
                            to_zero = false;
                            first_to_zero = true;
                        }
                    }
                }
            }
        }

        let mut paths = vec![];
        for nxt in expanded {
            if let Ok(path) = Self::process_path(nxt, input_token) {
                paths.push(path)
            }
        }
        // sort by path length
        paths.sort_by(|a, b| if a.len() > b.len() { Ordering::Greater } else { Ordering::Less });

        Self {
            input_token: input_token.clone(),
            paths,
            pools: pools.clone(),
        }
    }

    fn print_balance(balance: &HashMap<String, HashMap<String, I256>> ) {
        let mut fin = "".to_string();
        for (pl, bal) in balance {
            fin += &("\n".to_string() + pl);
            for (token, b) in bal {
                fin += &("\n\t-> ".to_string() + token + " == " + &b.to_string())
            }

        }
        debug!("{}\n", fin);
    }
    fn estimate_gas_unit_cost(path: Vec<MevPathStep>) -> u64 {
        1
    }
    fn validate_path(&self, mut path: Vec<MevPathStep>) -> anyhow::Result<String> {
        // binary search for optimal input
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = if !path.first().unwrap().is_exact_in() && path.first().unwrap().get_pool().supports_callback_payment() {
            MAX_SIZE.clone() / 2.0
        } else {
            MAX_SIZE.clone() / 2.0

        };
        let mut left = 0.0;
        let mut right = mid * 2.0;
        let decimals = crate::decimals(self.input_token.clone());
        let mut instructions = vec![];


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
        debug!("\n\n\n");
        'binary_search: for i in 0..1 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;

            let mut balance: HashMap<String, HashMap<String, I256>> = HashMap::new();
            let contract_address = "<contract_address>".to_string();

            // initially fill balances with 0 and contract wallet balance with current size
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


            let mut instruction = Vec::with_capacity(path.len());
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
                    debug!("_Step: {} {} {}", index, j, step);

                    match &step {
                        MevPathStep::ExactIn(pool, input, out) | MevPathStep::ExactOut(pool, input, out) => {
                            let asset_reciever = out.target.clone();

                            let (asset_token, debt_token) = if pool.x_to_y {
                                (pool.y_address.clone(), pool.x_address.clone())
                            } else {
                                (pool.x_address.clone(), pool.y_address.clone())
                            };
                            debug!("Recipient: {} Asset_Token: {} Debt_Token: {} ", asset_reciever, asset_token, debt_token);

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
                                    debug!("Casting Error: Cast To Uint {}", debt);

                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                let asset = I256::from_dec_str(&calculator.calculate_out(as_uint.unwrap(), pool)?.to_string()).unwrap();
                                a = asset;
                                d = debt;
                                debug!("Type PrePaid > Asset {} Debt {}", a, d);

                                let bal = *balance
                                    .get(&pool.address)
                                    .unwrap()
                                    .get(&debt_token)
                                    .unwrap();
                                balance
                                    .get_mut(&pool.address)
                                    .unwrap()
                                    .insert(debt_token, sub_i256(bal, debt));
                                let bal = *balance
                                    .get(&asset_reciever)
                                    .unwrap()
                                    .get(&asset_token)
                                    .unwrap();
                                balance
                                    .get_mut(&asset_reciever)
                                    .unwrap()
                                    .insert(asset_token, bal + (asset));
                            }
                            else if let Some((from, b)) = balance.iter().find(|(p, b)| {
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
                                    debug!("Casting Error: Cast To Uint {}", asset);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                let debt = I256::from_dec_str(&calculator.calculate_in(as_uint.unwrap(), pool)?.to_string()).unwrap();
                                a = asset;
                                d = debt;
                                debug!("Type AssetIsDebtedToOther > Asset {} Debt {}", a, d);

                                if !step.get_pool().supports_callback_payment() {
                                    let contract_balance = *balance
                                        .get(&contract_address)
                                        .unwrap()
                                        .get(&debt_token)
                                        .unwrap();
                                    if contract_balance <= I256::from(0) {
                                        debug!("Unproccessable AssetIsDebtedToOther step");
                                        return Err(anyhow::Error::msg("Unproccessable AssetIsDebtedToOther Step"))
                                    }

                                    balance
                                        .get_mut(&contract_address)
                                        .unwrap()
                                        .insert(debt_token.clone(), sub_i256(contract_balance, debt));
                                    balance
                                        .get_mut(&pool.address)
                                        .unwrap()
                                        .insert(debt_token, I256::from(0));
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
                                        .insert(debt_token, sub_i256(bal, debt));
                                }

                                let bal = *balance
                                    .get(&asset_reciever)
                                    .unwrap()
                                    .get(&asset_token)
                                    .unwrap();
                                balance
                                    .get_mut(&asset_reciever)
                                    .unwrap()
                                    .insert(asset_token, bal + (asset));
                            }


                            else if let Some((from, b)) = balance.iter().find(|(p, b)| {
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
                                    debug!("Casting Error: Cast To Uint {}", debt);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                let asset = I256::from_dec_str(&calculator.calculate_out(as_uint.unwrap(), pool)?.to_string()).unwrap();
                                a = asset;
                                d = debt;
                                debug!("Type DebtIsAssetOnSelf > Asset {} Debt {}", a, d);

                                let bal = *balance
                                    .get(&pool.address)
                                    .unwrap()
                                    .get(&debt_token)
                                    .unwrap();
                                balance
                                    .get_mut(&pool.address)
                                    .unwrap()
                                    .insert(debt_token, sub_i256(bal, debt));
                                let bal = *balance
                                    .get(&asset_reciever)
                                    .unwrap()
                                    .get(&asset_token)
                                    .unwrap();
                                balance
                                    .get_mut(&asset_reciever)
                                    .unwrap()
                                    .insert(asset_token, bal + (asset));
                            }


                            else if debt_token == self.input_token {
                                let debt = I256::from(i_atomic as u128);

                                // add the debt to current pool
                                let as_uint = U256::from_dec_str(&debt.to_string());
                                if as_uint.is_err() {
                                    debug!("Casting Error: Cast To Uint {}", debt);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                let asset = I256::from_dec_str(&calculator.calculate_out(as_uint.unwrap(), pool)?.to_string()).unwrap();
                                a = asset;
                                d = debt;
                                debug!("Type DebtIsInputToken > Asset {} Debt {}", a, d);
                                if !step.get_pool().supports_callback_payment() {

                                    balance
                                        .get_mut(&contract_address)
                                        .unwrap()
                                        .insert(debt_token.clone(), - debt);
                                    balance
                                        .get_mut(&pool.address)
                                        .unwrap()
                                        .insert(debt_token, I256::from(0));
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
                                        .insert(debt_token, sub_i256(bal, debt));
                                }

                                let bal = *balance
                                    .get(&asset_reciever)
                                    .unwrap()
                                    .get(&asset_token)
                                    .unwrap();
                                balance
                                    .get_mut(&asset_reciever)
                                    .unwrap()
                                    .insert(asset_token, bal + (asset));
                            }

                            else if asset_token == self.input_token && step.get_pool().supports_callback_payment() {
                                let asset = I256::from(i_atomic as u128);

                                let as_uint = U256::from_dec_str(&asset.to_string());
                                if as_uint.is_err() {
                                    debug!("Casting Error: Cast To Uint {}", asset);
                                    return Err(anyhow::Error::msg("Casting Error"));
                                }
                                let debt = I256::from_dec_str(&calculator.calculate_in(as_uint.unwrap(), pool)?.to_string()).unwrap();
                                a = asset;
                                d = debt;
                                debug!("Type AssetIsInputToken > Asset {} Debt {}", a, d);

                                // update asset reciever balance
                                let bal = *balance
                                    .get(&pool.address)
                                    .unwrap()
                                    .get(&debt_token)
                                    .unwrap();
                                balance
                                    .get_mut(&pool.address)
                                    .unwrap()
                                    .insert(debt_token, sub_i256(bal, debt));
                                let bal = *balance
                                    .get(&asset_reciever)
                                    .unwrap()
                                    .get(&asset_token)
                                    .unwrap();
                                balance
                                    .get_mut(&asset_reciever)
                                    .unwrap()
                                    .insert(asset_token, bal + (asset));
                            }
                            else if asset_token == self.input_token && index == 0 {
                                debug!("Unproccessable step");
                                return Err(anyhow::Error::msg("Unproccessable Step"))
                            }

                            else {
                                debug!("Can not process step");
                                continue 'inner;
                            }

                            if step.is_exact_in() {
                                match pool.provider.id() {
                                    LiquidityProviderId::UniswapV2 | LiquidityProviderId::SushiSwap  => {
                                        // update with reserves
                                        let (function, pay_to) = if sender == asset_reciever {
                                            (UNISWAP_V2_EXACT_IN_PAY_TO_SENDER.to_string(), "".to_string())
                                        } else if asset_reciever != contract_address {
                                            (UNISWAP_V2_EXACT_IN_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string())
                                        } else {
                                            (UNISWAP_V2_EXACT_IN_PAY_TO_SELF.to_string(), "".to_string())
                                        };
                                        let packed_asset = Self::encode_packed(a);
                                        let packed_debt = Self::encode_packed(d);
                                        instruction.push(
                                            function +
                                                if pool.x_to_y { "01" } else { "00" } +
                                                &pool.address[2..] +
                                                &dt[2..] +
                                                &pay_to +
                                                &(packed_asset.len() as u8).encode_hex()[64..] +
                                                &packed_asset +
                                                &packed_debt
                                        )
                                    }
                                    LiquidityProviderId::UniswapV3 => {
                                        // update with ...
                                        let (function, pay_to) = if sender == asset_reciever {
                                            (UNISWAP_V3_EXACT_IN_PAY_TO_SENDER.to_string(), "".to_string())
                                        } else if asset_reciever != contract_address {
                                            (UNISWAP_V3_EXACT_IN_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string())
                                        } else {
                                            (UNISWAP_V3_EXACT_IN_PAY_TO_SELF.to_string(), "".to_string())
                                        };
                                        instruction.push(
                                            function +
                                                if pool.x_to_y { "01" } else { "00" } +
                                                &pool.address[2..] +
                                                &pay_to +
                                                &Self::encode_packed(d)
                                        )
                                    }
                                }
                            } else {
                                match pool.provider.id() {
                                    LiquidityProviderId::UniswapV2 | LiquidityProviderId::SushiSwap => {
                                        // update with reserves
                                        let (function, pay_to) = if sender == asset_reciever {
                                            (UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                        } else if asset_reciever != contract_address {
                                            (UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string())
                                        } else {
                                            (UNISWAP_V2_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                        };
                                        let packed_asset = Self::encode_packed(a);
                                        let packed_debt = Self::encode_packed(d);
                                        instruction.push(
                                            function +
                                                if pool.x_to_y { "01" } else { "00" } +
                                                &pool.address[2..] +
                                                &dt[2..] +
                                                &pay_to +
                                                &(packed_asset.len() as u8).encode_hex()[64..] +
                                                &packed_asset +
                                                &packed_debt
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
                                        instruction.push(
                                            function +
                                                if pool.x_to_y { "01" } else { "00" } +
                                                &pool.address[2..] +
                                                &pay_to +
                                                &Self::encode_packed(a)
                                        )
                                    }
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
                            debug!("Amount For Payment: {}", amount_to_pay);
                            let (function, pay_to) = if sender == pool.address {
                                if *balance.get(&sender).unwrap().get(&token).unwrap() > I256::from(0) {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;
                                } else {
                                    // debug!("enough SENDER: {} {}", amount_to_pay.to_string(), balance.get(&sender).unwrap().get(&token).unwrap());

                                    let bal = *balance
                                        .get(&sender)
                                        .unwrap()
                                        .get(&token)
                                        .unwrap();
                                    debug!("Due Balance: {}", bal);
                                    debug!("Left: {}", sub_i256(amount_to_pay, bal));
                                    balance.get_mut(&contract_address).unwrap().insert(token.clone(), sub_i256(amount_to_pay, bal));

                                    amount_to_pay = bal;
                                    balance
                                        .get_mut(&sender)
                                        .unwrap()
                                        .insert(token.clone(), I256::from(0));
                                }

                                (PAY_SENDER.to_string(), "".to_string())
                            } else {
                                if *balance.get(&pool.address).unwrap().get(&token).unwrap() > I256::from(0) {
                                    right = mid;
                                    mid = (left + right) / 2.0;
                                    continue 'binary_search;
                                } else {
                                    // debug!("enough: {} {}", amount_to_pay.to_string(), balance.get(&pool.address).unwrap().get(&token).unwrap());

                                    let bal = *balance
                                        .get(&pool.address)
                                        .unwrap()
                                        .get(&token)
                                        .unwrap();
                                    debug!("Due Balance: {}", bal);
                                    debug!("Left: {}", sub_i256(amount_to_pay, bal));

                                    balance.get_mut(&contract_address).unwrap().insert(token.clone(), sub_i256(amount_to_pay, bal));

                                    amount_to_pay = bal;
                                    balance
                                        .get_mut(&pool.address)
                                        .unwrap()
                                        .insert(token.clone(), I256::from(0));
                                }
                                (PAY_ADDRESS.to_string(), pool.address.clone()[2..].to_string())
                            };

                            instruction.push(function + &token[2..] + &pay_to + &Self::encode_packed(amount_to_pay));
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
            let asset_balances = balance.iter().map(|(p, b)| b.get(&self.input_token).unwrap()).map(|b| if *b < I256::from(0) { -*b } else { *b }).filter(|b| b != &I256::from(i_atomic as u128)).sorted().collect::<Vec<I256>>();
            if steps_done.len() != path.len() {
                warn!("{} {}", steps_done.len(), path.len());
                return Err(anyhow::Error::msg("Invalid Path: Not All Steps Done in Path"));
            } else {

                let mut final_balance = *balance.get(&contract_address).unwrap().get(&self.input_token).unwrap();
                let profit = sub_i256(final_balance, I256::from(i_atomic as u128));
                debug!("profit {}, iatomic {} ", final_balance.as_i128() as f64 / 10_f64.powf(18.0), i_atomic);
                Self::print_balance(&balance);

                if i == 0 {
                    best_route_profit = final_balance;
                    best_route_size = i_atomic;
                }
                if final_balance >= best_route_profit {
                    best_route_profit = final_balance;
                    best_route_size = i_atomic;
                    best_route_index = i;
                    left = mid;
                } else {
                    right = mid;
                }
                mid = (left + right) / 2.0;
            }
        }

        info!("Size: {} Profit: {}\n{} {}", best_route_size / 10_f64.powf(18.0), best_route_profit.as_i128() as f64 / 10_f64.powf(18.0),path.len(),Self::path_to_solidity_test(&path, &instructions[best_route_index]));
        for step in &path {
            info!("{} -> {}", step, step.get_output());
        }
        info!("\n\n\nDone path\n\n\n");
        if best_route_profit > I256::from(0) {

            let mut final_data = "".to_string();
            for ix in instructions[best_route_index].clone() {
                let end = ix.len() as u8;
                final_data += &end.encode_hex()[64..];
                final_data += &ix;
            }
            Ok(final_data)
        } else if best_route_profit == I256::from(0) {
            Err(anyhow::Error::msg("Inv Path"))
        } else {
            Ok("NO PROFIT".to_string())
        }
    }

    fn path_to_solidity_test(path: &Vec<MevPathStep>, instructions: &Vec<String>) -> String {
        let mut builder = "".to_string();
        let mut title = "function test".to_string();

        let mut final_data = "".to_string();
        for ix in instructions.clone() {
            let end = ix.len() as u8;
            final_data += &end.encode_hex()[64..];
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
        builder += "\n\tvm.expectRevert();";
        builder += &format!("\n\tagg.functionName(data);\n}}");
        builder
    }

    fn function_type(function: String) -> String {
        let res = match function.as_str() {
            "0000004b" | "000000d0" | "00000015" | "000000cd" | "000000ea" => "PayToSender",
            "000000e1" | "000000fc" | "0000003c" | "00000082" => "PayToSelf",
            "000000c9" | "00000091" | "000000e5" | "00000059" | "00000081" => "PayToAddress",
            _ => ""
        };
        return res.to_string();
    }

    fn encode_packed(amount: I256) -> String {
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
    pub fn update(&mut self, updated_pool: Pool) -> MevPathUpdateResult {
        for path in self.paths.iter_mut() {
            // check if the path has positive outcome
            for mut step in (*path).iter_mut() {
                // update first
                if step.contains_pool(&updated_pool) {
                    step.update_pool(&updated_pool);
                }
            }
        }

        for (index, path) in self
            .paths
            .iter()
            .enumerate()
            .collect::<Vec<(usize, &Vec<MevPathStep>)>>()
        {
            // TODO: use transaction builder

        }

        MevPathUpdateResult {
            transactions: vec![],
        }
    }
    pub fn get_transactions(&self) {
        for (index, path) in self
            .paths
            .iter()
            .enumerate()
            .collect::<Vec<(usize, &Vec<MevPathStep>)>>()
        {
            let is_good = self.validate_path(path.clone());
            match &is_good {
                Ok(data) => {
                    if data == "NO PROFIT" {
                        continue
                    } else {
                        println!("{}", data)
                    }
                },
                Err(e) => {
                    error!("{:?}", e);

                }
            }

        }
    }

    pub fn is_valid(&self) -> bool {
        if self.paths.len() <= 0 {
            return false;
        }

        let is_good = self.validate_path(self.paths.get(0).unwrap().clone());
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
            if asset_token == *input_token  {
                balance1.get_mut(&pool).unwrap().insert(asset_token.clone(), true);
            }
            else if let Some(pay_to_step) = step_stack.iter().find(|s| {
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
