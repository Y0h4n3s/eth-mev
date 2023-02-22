use std::collections::HashMap;
use std::fmt::{Debug, Display};

use backtrack::problem::{Check, Scope};
use backtrack::solvers::IterSolveCached;
use ethers::types::{Eip1559TransactionRequest, U128};
use garb_sync_eth::{
    uniswap_v2::UniswapV2Metadata, uniswap_v3::UniswapV3Metadata, LiquidityProviderId,
    LiquidityProviders, Pool, PoolInfo, UniswapV3Calculator,
};
// helper trait to filter solutions of interest
use crate::MAX_SIZE;
use backtrack::solve::IterSolveExt;
use ethers::abi::AbiEncode;
use itertools::Itertools;
use num_traits::cast::ToPrimitive;
use ethers::types::{U256, I256};
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

pub struct ExpandedPath {
    pub path: Vec<MevPathStep>,
    pub size: usize,
    pub in_token: String,
}

impl ExpandedPath {
    pub fn new(pools: &Vec<Pool>, in_token: &String) -> Self {
        Self {
            path: pools
                .iter()
                .cloned()
                .map(|pool| {
                    let input = StepInput::default();
                    let output = StepOutput::default();
                    let mut other = pool.clone();
                    other.x_to_y = !other.x_to_y;
                    vec![
                        MevPathStep::ExactIn(other.clone(), input.clone(), output.clone()),
                        MevPathStep::ExactIn(pool.clone(), input.clone(), output.clone()),
                        MevPathStep::ExactOut(other, input.clone(), output.clone()),
                        MevPathStep::ExactOut(pool, input.clone(), output.clone()),
                    ]
                })
                .flatten()
                .collect(),
            size: pools.len(),
            in_token: in_token.clone(),
        }
    }
}

impl Scope<'_, MevPathStep> for ExpandedPath {
    fn len(&self) -> usize {
        self.path.len()
    }
    fn size(&self) -> usize {
        self.size
    }
    fn value(&'_ self, index: usize) -> MevPathStep {
        self.path.get(index).unwrap().clone()
    }
}

impl Check<MevPathStep> for ExpandedPath {
    fn extends_sat(&self, solution: &[MevPathStep], x: &MevPathStep) -> bool {
        if solution
            .iter()
            .find(|p| p.get_pool().eq(&x.get_pool()))
            .is_some()
        {
            return false;
        }

        if solution.len() < self.size - 1 {
            return true;
        }

        let mut debt = vec![];
        let mut asset = vec![];
        let mut proposed = Vec::from(solution);
        proposed.push(x.clone());
        let first = proposed.first().unwrap();
        for step in proposed.iter() {
            let (asset_token, debt_token) = match step {
                MevPathStep::ExactIn(pool, _, _) | MevPathStep::ExactOut(pool, _, _) => {
                    if pool.x_to_y {
                        (pool.y_address.clone(), pool.x_address.clone())
                    } else {
                        (pool.x_address.clone(), pool.y_address.clone())
                    }
                }
                // we never reach here
                _ => ("".to_string(), "".to_string()),
            };
            debt.push((step, debt_token));
            if let Some((index, _)) = debt.iter().find_position(|(p, t)| {
                t == &asset_token && p.get_pool().address != step.get_pool().address
            }) {
                debt.remove(index);
            } else {
                asset.push((step, asset_token));
            }
        }

        for (pool, asset_token) in asset {
            if let Some((index, _)) = debt.iter().find_position(|(_p, t)| t == &asset_token) {
                debt.remove(index);
            }
        }
        //        if debt.len() == 0 && proposed.len() == 4 {
        //            println!("`````````````````````` Tried Route ``````````````````````");
        //            for (i, pool) in proposed.iter().enumerate() {
        //                println!("{}. {}", i + 1, pool);
        //            }
        //        }
        return debt.len() == 0 && proposed.len() == self.size;
    }
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct MevPath {
    pub paths: Vec<Vec<MevPathStep>>,
    pub pools: Vec<Pool>,
    pub input_token: String,
}
#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct StepOutputTarget {
    pool: Option<Pool>,
    address: Option<String>,
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct  StepInput {
    pub function_hash: String,
    pub pay_to: String,
    pub amount: u128
}
#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct StepOutput {
    target: StepOutputTarget,
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

impl MevPath {
    pub fn new(pools: &Vec<Pool>, input_token: &String) -> Self {
        let expanded = ExpandedPath::new(pools, input_token);
        let solver = IterSolveCached::new(&expanded);
        let mut sats = solver.sat_iter();
        let mut paths = vec![];
        while let Some(nxt) = sats.next() {
            //            println!("`````````````````````` Tried Route ``````````````````````");
            //                        for (i, pool) in nxt.iter().enumerate() {
            //                            println!("{}. {}", i + 1, pool);
            //                        }

            if let Ok(path) = Self::process_path(nxt, input_token) {
                mev_path_to_forge_test_title!("df");
                paths.push(path)
            }
        }
        Self {
            input_token: input_token.clone(),
            paths,
            pools: pools.clone(),
        }
    }

    fn validate_path(&self, mut path: Vec<MevPathStep>) -> anyhow::Result<bool> {
        // binary search for optimal input
        let mut best_route_size = 0.0;
        let mut best_route_profit = I256::from(0);
        let mut best_route_index = 0;
        let mut mid = MAX_SIZE.clone() / 2.0;
        let mut left = 0.0;
        let mut right = MAX_SIZE.clone();
        let decimals = crate::decimals(self.input_token.clone());
        let mut instructions = vec![];
        // println!("Checking Path: {:?}", path);
        'checker: for i in 0..5 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let contract_address = "<contract_address>".to_string();
            // this map should have structure like <Address, <token, balance>>
            // and i should be using step input and step output
            let mut balance: HashMap<String, HashMap<String, I256>> = HashMap::new();
            let mut instruction = vec![];
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
            balance
                .get_mut(&contract_address)
                .unwrap()
                .insert(self.input_token.clone(), I256::from(i_atomic as u128));

            let mut sender = contract_address.clone();
            for mut step in path.iter_mut() {
//                 println!("Doing Step: {}", step);
//                 println!("Balances: {:?}", balance);
                match step {
                    MevPathStep::ExactIn(pool, input, out) => {
                        let asset_reciever = if out.target.pool.is_some() {
                            out.target.pool.as_ref().unwrap().address.clone()
                        } else {
                            out.target.address.as_ref().unwrap().clone()
                        };
                        let (asset_token, debt_token) = if pool.x_to_y {
                            (pool.y_address.clone(), pool.x_address.clone())
                        } else {
                            (pool.x_address.clone(), pool.y_address.clone())
                        };

                        let calculator = pool.provider.build_calculator();
                        let mut debt = *balance
                            .get(&contract_address)
                            .unwrap()
                            .get(&debt_token)
                            .unwrap();
                        // check forward payment availablity here
                        // if there isn't and debt balance is < debt continue to next size

                        if debt <= I256::from(0) {
                            if !pool.supports_callback_payment() {
                                continue 'checker;
                            //                            return Err(anyhow::Error::msg(
                            //                                "Not enough balance to complete transaction",
                            //                            ));
                            } else {
                                let bal = balance
                                    .get(&contract_address)
                                    .unwrap()
                                    .get(&asset_token)
                                    .unwrap();
                                if bal <= &I256::from(0) {
                                    return Err(anyhow::Error::msg(
                                        "No asset or debt balance for swap",
                                    ));
                                }

                                let out =
                                    I256::from_dec_str(&calculator.calculate_in(U256::from_dec_str(&bal.to_string()).unwrap(), pool)?.to_string()).unwrap();
                                debt = out;
                            }
                        }
                        let as_uint = U256::from_dec_str(&debt.to_string());
                        if as_uint.is_err() {
                            return Err(anyhow::Error::msg("Casting Error"));
                        }
                        let asset = I256::from_dec_str(&calculator.calculate_out(as_uint.unwrap(), pool)?.to_string()).unwrap();
                        let bal = *balance
                            .get(&pool.address)
                            .unwrap()
                            .get(&debt_token)
                            .unwrap();
                        balance
                            .get_mut(&pool.address)
                            .unwrap()
                            .insert(debt_token, bal - (debt));


                        let bal = *balance
                            .get(&asset_reciever)
                            .unwrap()
                            .get(&asset_token)
                            .unwrap();
                        balance
                            .get_mut(&asset_reciever)
                            .unwrap()
                            .insert(asset_token, bal + (asset));
//                         println!("Debt: {:?} Asset: {:?}", debt, asset_i128);
                        match pool.provider.id() {
                            LiquidityProviderId::UniswapV2 => {
                                // update with reserves
                                let (function, pay_to) = if sender == asset_reciever {
                                    (UNISWAP_V2_EXACT_IN_PAY_TO_SENDER.to_string(), "".to_string())
                                } else if asset_reciever != contract_address {
                                    (UNISWAP_V2_EXACT_IN_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string())
                                } else {
                                    (UNISWAP_V2_EXACT_IN_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                instruction.push(function + &pool.address[2..] + &pay_to + &Self::encode_packed(asset))
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
                                instruction.push(function + &pool.address[2..] + &pay_to + &Self::encode_packed(asset))
                            }
                        }
                        sender = pool.address.clone()

                    }
                    MevPathStep::ExactOut(pool, input, out) => {
                        let asset_reciever = if out.target.pool.is_some() {
                            out.target.pool.as_ref().unwrap().address.clone()
                        } else {
                            out.target.address.as_ref().unwrap().clone()
                        };
                        let (asset_token, debt_token) = if pool.x_to_y {
                            (pool.y_address.clone(), pool.x_address.clone())
                        } else {
                            (pool.x_address.clone(), pool.y_address.clone())
                        };
                        let calculator = pool.provider.build_calculator();
                        let mut asset = *balance
                            .get(&asset_reciever)
                            .unwrap()
                            .get(&asset_token)
                            .unwrap();
                        if asset == I256::from(0) {
                            let bal = balance
                                .get(&contract_address)
                                .unwrap()
                                .get(&debt_token)
                                .unwrap();
                            if bal <= &I256::from(0) {
                                return Err(anyhow::Error::msg(
                                    "No asset or debt balance for swap",
                                ));
                            }

                            let out =
                                    I256::from_dec_str(&calculator.calculate_out(U256::from_dec_str(&bal.to_string()).unwrap(), pool)?.to_string()).unwrap();
                            asset = out;
                        } else if asset < I256::from(0) {
                            asset = -asset;
                        }
                        let as_uint = U256::from_dec_str(&asset.to_string());
                        if as_uint.is_err() {
                            return Err(anyhow::Error::msg("Casting Error"));
                        }
                        let debt = I256::from_dec_str(&calculator.calculate_in(as_uint.unwrap(), pool)?.to_string()).unwrap();
                        // check forward payment availablity here
                        // if there isn't and debt balance is < debt continue to next size


                        let bal = *balance
                            .get(&asset_reciever)
                            .unwrap()
                            .get(&asset_token)
                            .unwrap();
                        balance
                            .get_mut(&asset_reciever)
                            .unwrap()
                            .insert(asset_token, bal + (asset));

                        let bal = *balance
                            .get(&pool.address)
                            .unwrap()
                            .get(&debt_token)
                            .unwrap();
                        balance
                            .get_mut(&pool.address)
                            .unwrap()
                            .insert(debt_token, bal - (debt));
                        match pool.provider.id() {
                            LiquidityProviderId::UniswapV2 => {
                                // update with reserves
                                let (function, pay_to) = if sender == asset_reciever {
                                    (UNISWAP_V2_EXACT_OUT_PAY_TO_SENDER.to_string(), "".to_string())
                                } else if asset_reciever != contract_address {
                                    (UNISWAP_V2_EXACT_OUT_PAY_TO_ADDRESS.to_string(), asset_reciever[2..].to_string())
                                } else {
                                    (UNISWAP_V2_EXACT_OUT_PAY_TO_SELF.to_string(), "".to_string())
                                };
                                instruction.push(function + &pool.address[2..] + &pay_to + &Self::encode_packed(asset))
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
                                instruction.push(function + &pool.address[2..] + &pay_to + &Self::encode_packed(asset))

                            }
                        }
//                         println!("Debt: {:?} Asset: {:?}", debt_i128, asset);
                        sender = pool.address.clone()
                    }

                    // skip for nwo
                    MevPathStep::Payback(pool, input, is_x) => {
                        let token = if *is_x {
                            pool.x_address.clone()
                        } else {
                            pool.y_address.clone()
                        };
                        let (function, pay_to) = if sender == pool.address {
                            (PAY_SENDER.to_string(), "".to_string())
                        } else {
                            (PAY_ADDRESS.to_string(), pool.address.clone()[2..].to_string())
                        };

                        instruction.push(function + &pay_to + &Self::encode_packed(*balance.get_mut(&pool.address).unwrap().get(&token).unwrap()));

                        balance.get_mut(&pool.address).unwrap().insert(token, I256::from(0));
                    }
                }
            }
            instructions.push(instruction);

            let profit = *balance
                .get(&contract_address)
                .unwrap()
                .get(&self.input_token)
                .unwrap()
                         - I256::from(i_atomic as u128);

            if balance
                .into_values()
                .into_iter()
                .any(|bals| bals.into_iter().find(|(_, bal)| *bal < I256::from(0)).is_some())
            {
                continue;
            }

            if i == 0 {
                best_route_profit = profit;
                best_route_size = i_atomic;
            }
            if profit > best_route_profit {
                best_route_profit = profit;
                best_route_size = i_atomic;
                best_route_index = i;
                left = mid;
            } else {
                right = mid;
            }
            mid = (left + right) / 2.0;
        }
        if best_route_profit > I256::from(0) {
           println!("{}", Self::path_to_solidity_test(&path, &instructions[best_route_index]));


            Ok(true)
        } else {
            Ok(false)
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
                    title += &format!("_{}{}","Payback", &Self::function_type(function));
                }
            }
        }
        builder += &(title + "() public {\n");
        builder += "\tint256 amountIn = 50000000000000;";
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
        return res.to_string()
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
                continue
            } else {
                break
            }
        }
        let data = if encoded[index..].len() % 2 == 0 {
            return encoded[index..].to_string()
        } else {
            return "0".to_string() + &encoded[index..]
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

//        println!("Trying updated paths");
        let mut remove_indexes = vec![];
        for (index, path) in self
            .paths
            .iter()
            .enumerate()
            .collect::<Vec<(usize, &Vec<MevPathStep>)>>()
        {
            let mut path = path.to_vec();
            let is_good = self.validate_path(path);
            match &is_good {
                Ok(_) => (),
                Err(e) => {
                    remove_indexes.push(index);
                }
            }
        }

//        println!("Before: {:?}", self.paths.len());
        remove_indexes.reverse();
        for index in remove_indexes {
            self.paths.remove(index);
        }
//        println!("After: {:?}", self.paths.len());


        MevPathUpdateResult {
            transactions: vec![],
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

        let mut debt = vec![];
        let mut asset = vec![];
        let mut in_token = input_token.clone();
        asset.push((first_step, in_token));
        for step in path.iter_mut() {
            let exact_in = step.is_exact_in();
            let (asset_token, debt_token) = match step {
                MevPathStep::ExactIn(pool, _, _) | MevPathStep::ExactOut(pool, _, _) => {
                    if pool.x_to_y {
                        (pool.y_address.clone(), pool.x_address.clone())
                    } else {
                        (pool.x_address.clone(), pool.y_address.clone())
                    }
                }
                // we never reach here
                _ => ("".to_string(), "".to_string()),
            };
            debt.push((step.clone(), debt_token.clone()));
            let step_out = if let Some((index, (to, _token))) =
                debt.iter().find_position(|(p, t)| &asset_token == t)
            {
                let out = StepOutput {
                    target: StepOutputTarget {
                        pool: Some(to.get_pool()),
                        address: None,
                    },
                };
                debt.remove(index);
                out
            } else {
                StepOutput {
                    target: StepOutputTarget {
                        pool: None,
                        address: Some("<contract_address>".to_string()),
                    },
                }
            };
            step.update_output(&step_out);
            if exact_in {
                // if we don't have the current input from previous steps stop and current step doesn't support callback payment
                if let Some((index, token)) = asset.iter().find_position(|(p, a)| a == &debt_token)
                {
                    asset.remove(index);
                } else {
                    if !step.get_pool().supports_callback_payment() {
                        return Err(anyhow::Error::msg(
                            "Invalid Step: No asset for successful swap",
                        ));
                    }
                }
                step_stack.push(step.clone());
            } else {
                step_stack.push(step.clone());
            }
            in_token = asset_token.clone();
            asset.push((step.clone(), asset_token));
        }

        for (pool, asset_token) in asset {
            if let Some((index, (p, token))) =
                debt.iter().find_position(|(_p, t)| t == &asset_token)
            {
                let pl = p.get_pool();
                step_stack.push(MevPathStep::Payback(
                    pl.clone(),
                    StepInput::default(),
                    &pl.x_address == token,
                ));
                debt.remove(index);
            }
        }

        // this should already be filtered out but just in case
        if debt.len() != 0 {
            return Err(anyhow::Error::msg("Not a valid path"));
        }
        Ok(step_stack)
    }
}

// keep this logic seprate
struct TransactionBuilder {}
