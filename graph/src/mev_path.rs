use std::collections::HashMap;
use std::fmt::{Debug, Display};

use backtrack::problem::{Check, Scope};
use backtrack::solvers::IterSolveCached;
use ethers::types::Eip1559TransactionRequest;
use garb_sync_eth::{
    uniswap_v2::UniswapV2Metadata, uniswap_v3::UniswapV3Metadata, LiquidityProviderId,
    LiquidityProviders, Pool, PoolInfo, UniswapV3Calculator,
};
// helper trait to filter solutions of interest
use crate::MAX_SIZE;
use backtrack::solve::IterSolveExt;
use itertools::Itertools;
use num_traits::cast::ToPrimitive;

const MINIMUM_PATH_LENGTH: usize = 2;
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
                    let input = StepInput::get_default_for_id(pool.provider.id());
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
pub struct UniswapV3Input {
    pub function_hash: String,
}
#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct UniswapV2Input {
    pub function_hash: String,

}

#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum StepInput {
    UniswapV3Input(UniswapV3Input),
    UniswapV2Input(UniswapV2Input),
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
impl StepInput {
    pub fn get_default_for_id(id: LiquidityProviderId) -> Self {
        match id {
            LiquidityProviderId::UniswapV2 => Self::UniswapV2Input(Default::default()),
            LiquidityProviderId::UniswapV3 => Self::UniswapV3Input(Default::default()),
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
    pub fn serialize(&self) -> String {
        match self {
            MevPathStep::ExactIn(pool, input, output) => {
                if pool.provider.id() == LiquidityProviderId::UniswapV2 {
                    "0x000000".to_string() + &pool.address
                } else {
                    "0x000001".to_string() + &pool.address
                }
            }
            MevPathStep::ExactOut(pool, input, output) => {
                if pool.provider.id() == LiquidityProviderId::UniswapV2 {
                    "0x000002".to_string() + &pool.address
                } else {
                    "0x000003".to_string() + &pool.address
                }
            }
            MevPathStep::Payback(pool, input, is_x) => {
                if pool.provider.id() == LiquidityProviderId::UniswapV2 {
                    "0x000004".to_string() + &pool.address
                } else {
                    "0x000005".to_string() + &pool.address
                }
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

    fn validate_path(&self, path: &Vec<MevPathStep>) -> anyhow::Result<bool> {
        // binary search for optimal input
        let mut best_route_size = 0.0;
        let mut best_route_profit = 0;
        let mut mid = MAX_SIZE.clone() / 2.0;
        let mut left = 0.0;
        let mut right = MAX_SIZE.clone();
        let decimals = crate::decimals(self.input_token.clone());
        let mut instructions = vec![];
        // println!("Checking Path: {:?}", path);
        'checker: for i in 0..5 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let in_ = i_atomic as u128;
            let contract_address = "<contract_address>".to_string();
            // this map should have structure like <Address, <token, balance>>
            // and i should be using step input and step output
            let mut balance: HashMap<String, HashMap<String, i128>> = HashMap::new();
            for step in path.iter_mut() {
                let pool = step.get_pool();

                for step in path {
                    let pool1 = step.get_pool();
                    if balance.get(&pool.address).is_some() {
                        balance
                            .get_mut(&pool.address)
                            .unwrap()
                            .insert(pool1.x_address, 0);
                        balance
                            .get_mut(&pool.address)
                            .unwrap()
                            .insert(pool1.y_address, 0);
                    } else {
                        let mut map = HashMap::new();
                        map.insert(pool1.x_address, 0);
                        map.insert(pool1.y_address, 0);
                        balance.insert(pool.address.clone(), map);
                    }
                }
                if balance.get(&contract_address).is_some() {
                    balance
                            .get_mut(&contract_address)
                            .unwrap()
                            .insert(pool.x_address, 0);
                    balance
                            .get_mut(&contract_address)
                            .unwrap()
                            .insert(pool.y_address, 0);
                } else {
                let mut map = HashMap::new();
                map.insert(pool.x_address, 0);
                map.insert(pool.y_address, 0);
                balance.insert(contract_address.clone(), map);
            }
            }
            balance
                .get_mut(&contract_address)
                .unwrap()
                .insert(self.input_token.clone(), i_atomic as i128);

            let mut sender = contract_address.clone();
            for mut step in path {
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

                        if debt <= 0 {
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
                                if bal <= &0 {
                                    return Err(anyhow::Error::msg(
                                        "No asset or debt balance for swap",
                                    ));
                                }

                                let out =
                                    calculator.calculate_in(bal.to_u128().unwrap(), pool)? as i128;
                                debt = out;
                            }
                        }
                        let as_uint = debt.to_u128();
                        if as_uint.is_none() {
                            return Err(anyhow::Error::msg("Casting Error"));
                        }
                        let asset = calculator.calculate_out(as_uint.unwrap(), pool)?;
                        let bal = *balance
                            .get(&pool.address)
                            .unwrap()
                            .get(&debt_token)
                            .unwrap();
                        balance
                            .get_mut(&pool.address)
                            .unwrap()
                            .insert(debt_token, bal - (debt));

                        let asset_i128 = asset.to_i128();
                        if asset_i128.is_none() {
                            return Err(anyhow::Error::msg("Casting Error"));
                        }

                        let bal = *balance
                            .get(&asset_reciever)
                            .unwrap()
                            .get(&asset_token)
                            .unwrap();
                        balance
                            .get_mut(&asset_reciever)
                            .unwrap()
                            .insert(asset_token, bal + (asset_i128.unwrap()));
//                         println!("Debt: {:?} Asset: {:?}", debt, asset_i128);
                        match pool.provider.id() {
                            LiquidityProviderId::UniswapV2 => {
                                // update with reserves
                                let function = if sender == pool.address {
                                    "uniswapV2ExactInPayToSender".to_string()
                                } else {
                                    "uniswapV2ExactIn".to_string()
                                }
                                step.update_input(UniswapV2Input{
                                    function_hash: function
                                })
                            }
                            LiquidityProviderId::UniswapV3 => {
                                // update with ...
                                let function = if sender == pool.address {
                                    "uniswapV3ExactInPayToSender".to_string()
                                } else {
                                    "uniswapV3ExactIn".to_string()
                                }
                                step.update_input(UniswapV3Input {
                                    function_hash: function
                                })
                            }
                        }
                        instructions.push(step.serialize())
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
                        if asset == 0 {
                            let bal = balance
                                .get(&contract_address)
                                .unwrap()
                                .get(&debt_token)
                                .unwrap();
                            if bal <= &0 {
                                return Err(anyhow::Error::msg(
                                    "No asset or debt balance for swap",
                                ));
                            }

                            let out =
                                calculator.calculate_out(bal.to_u128().unwrap(), pool)? as i128;
                            asset = out;
                        } else if asset < 0 {
                            asset = -asset;
                        }
                        let as_uint = asset.to_u128();
                        if as_uint.is_none() {
                            return Err(anyhow::Error::msg("Casting Error"));
                        }
                        let debt = calculator.calculate_in(as_uint.unwrap(), pool)?;
                        // check forward payment availablity here
                        // if there isn't and debt balance is < debt continue to next size
                        let debt_i128 = debt.to_i128();
                        if debt_i128.is_none() {
                            return Err(anyhow::Error::msg("Casting Error"));
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

                        let bal = *balance
                            .get(&pool.address)
                            .unwrap()
                            .get(&debt_token)
                            .unwrap();
                        balance
                            .get_mut(&pool.address)
                            .unwrap()
                            .insert(debt_token, bal - (debt_i128.unwrap()));
                        match pool.provider.id() {
                            LiquidityProviderId::UniswapV2 => {
                                // update with reserves
                                let function = if sender == pool.address {
                                    "uniswapV2ExactOutPayToSender".to_string()
                                } else {
                                    "uniswapV2ExactOut".to_string()
                                }
                                step.update_input(UniswapV2Input{
                                    function_hash: function
                                })
                            }
                            LiquidityProviderId::UniswapV3 => {
                                // update with ...
                                let function = if sender == pool.address {
                                    "uniswapV3ExactOutPayToSender".to_string()
                                } else {
                                    "uniswapV3ExactOut".to_string()
                                }
                                step.update_input(UniswapV3Input {
                                    function_hash: function
                                })
                            }
                        }
                        instructions.push(step.serialize())
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
                        balance.get_mut(&pool.address).unwrap().insert(token, 0);
                    }
                }
            }

            let profit = balance
                .get(&contract_address)
                .unwrap()
                .get(&self.input_token)
                .unwrap()
                - i_atomic as i128;

            if balance
                .into_values()
                .into_iter()
                .any(|bals| bals.into_iter().find(|(_, bal)| *bal < 0).is_some())
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
                left = mid;
            } else {
                right = mid;
            }
            mid = (left + right) / 2.0;
        }
        if best_route_profit > 0 {
//            println!("{} {} ", best_route_profit, best_route_size);
//            for step in path {
//                println!("{}", step);
//            }

            Ok(true)
        } else {
            Ok(false)
        }
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

        println!("Trying updated paths");
        let mut remove_indexes = vec![];
        for (index, path) in self
            .paths
            .iter()
            .enumerate()
            .collect::<Vec<(usize, &Vec<MevPathStep>)>>()
        {
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
                    StepInput::get_default_for_id(pl.provider.id()),
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
