use std::collections::HashMap;

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
        if solution.iter().find(|p| p.eq(&x)).is_some() {
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
    pub arrangements: Vec<Vec<MevPathStep>>,
    pub input_token: String,
}
#[derive(Hash, PartialEq, Eq, Debug, Clone, Default)]
pub struct StepOutputTarget {
    pool: Option<Pool>,
    address: Option<String>,
}

#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum StepInput {
    UniswapV3Input(UniswapV3Metadata),
    UniswapV2Input(UniswapV2Metadata),
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
        let mut arrangements = vec![];
        let mut paths = vec![];
        while let Some(nxt) = sats.next() {
            //            println!("`````````````````````` Tried Route ``````````````````````");
            //                        for (i, pool) in nxt.iter().enumerate() {
            //                            println!("{}. {}", i + 1, pool);
            //                        }

            if let Ok(path) = Self::process_path(&nxt, input_token) {
                mev_path_to_forge_test_title!("df");
                paths.push(path)
            }
            arrangements.push(nxt);
        }
        Self {
            arrangements,
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

        'checker: for i in 0..10 {
            let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
            let mut in_ = i_atomic as u128;
            let mut balance = HashMap::new();
            balance.insert(self.input_token.clone(), i_atomic as i128);
            for step in path {
                let pool = step.get_pool();
                balance.insert(pool.x_address, 0);
                balance.insert(pool.y_address, 0);
            }
            for mut step in path {
                match step {
                    MevPathStep::ExactIn(pool, input, out) => {
                        let (asset_token, debt_token) = if pool.x_to_y {
                            (pool.y_address.clone(), pool.x_address.clone())
                        } else {
                            (pool.x_address.clone(), pool.y_address.clone())
                        };

                        let calculator = pool.provider.build_calculator();
                        let debt = balance.get(&debt_token).unwrap();
                        // check forward payment availablity here
                        // if there isn't and debt balance is < debt continue to next size
                        if *debt <= 0 {
                            continue 'checker
//                            return Err(anyhow::Error::msg(
//                                "Not enough balance to complete transaction",
//                            ));
                        }
                        let as_uint = debt.to_u128();
                        if as_uint.is_none() {
                            return Err(anyhow::Error::msg(
                                    "Casting Error",
                            ));
                        }
                        let asset = calculator.calculate_out(as_uint.unwrap(), pool).unwrap();
                        if let Some(bal) = balance.get(&debt_token) {
                            balance.insert(debt_token, bal - (*debt));
                        } else {
                            balance.insert(debt_token, -(*debt));
                        }

                        let asset_i128 = asset.to_i128();
                        if asset_i128.is_none() {
                            return Err(anyhow::Error::msg(
                                    "Casting Error",
                            ));
                        }
                        if let Some(bal) = balance.get(&asset_token) {
                            balance.insert(asset_token, bal + (asset_i128.unwrap()));
                        } else {
                            balance.insert(asset_token, asset_i128.unwrap());
                        }
                    }
                    MevPathStep::ExactOut(pool, input, out) => {
                        let (asset_token, debt_token) = if pool.x_to_y {
                            (pool.y_address.clone(), pool.x_address.clone())
                        } else {
                            (pool.x_address.clone(), pool.y_address.clone())
                        };
                        let calculator = pool.provider.build_calculator();
                        let asset = balance.get(&asset_token).unwrap();
                        let as_uint = asset.to_u128();
                        if as_uint.is_none() {
                            return Err(anyhow::Error::msg(
                                    "Casting Error",
                            ));
                        }
                        let debt = calculator.calculate_in(as_uint.unwrap(), pool).unwrap();
                        // check forward payment availablity here
                        // if there isn't and debt balance is < debt continue to next size
                        let debt_i128 = debt.to_i128();
                        if debt_i128.is_none() {
                            return Err(anyhow::Error::msg(
                                    "Casting Error",
                            ));
                        }

                        if let Some(bal) = balance.get(&asset_token) {
                            balance.insert(asset_token, bal + (*asset));
                        } else {
                            balance.insert(asset_token, (*asset));
                        }

                        if let Some(bal) = balance.get(&debt_token) {
                            balance.insert(debt_token, bal - (debt_i128.unwrap()));
                        } else {
                            balance.insert(debt_token, -debt_i128.unwrap());
                        }
                    }

                    // skip for nwo
                    MevPathStep::Payback(pool, input, is_x) => {
                        let token = if *is_x {
                            pool.x_address.clone()
                        } else {
                            pool.y_address.clone()
                        };
                        balance.insert(token, 0);
                    }
                }
            }



            let profit = balance.get(&self.input_token).unwrap() - i_atomic as i128;
            if balance.into_values().into_iter().find(|bal| *bal < 0).is_some() {
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
             println!("Step {}: {} new mid {} ({} - {}) {} {}", i, profit, mid ,left, right, in_, i_atomic);
        }

        Ok(true)
    }
    pub fn update(&mut self, updated_pool: Pool) -> MevPathUpdateResult {
        for path in self.paths.iter_mut() {
            // check if the path has positive outcome
            for mut step in (*path).iter_mut() {
                // update first
                if step.contains_pool(&updated_pool) {
                    let update_meta = match updated_pool.provider {
                        LiquidityProviders::UniswapV2(ref meta) => {
                            StepInput::UniswapV2Input(meta.clone())
                        }
                        LiquidityProviders::UniswapV3(ref meta) => {
                            StepInput::UniswapV3Input(meta.clone())
                        }
                    };
                    step.update_input(&update_meta);
                }
            }
        }

        println!("Trying updated paths");
        for path in &self.paths {
            let is_good = self.validate_path(path);
            println!("{:?}", is_good);
        }

        MevPathUpdateResult {
            transactions: vec![],
        }
    }

    pub fn process_path(
        path: &Vec<MevPathStep>,
        input_token: &String,
    ) -> anyhow::Result<Vec<MevPathStep>> {
        if path.len() < MINIMUM_PATH_LENGTH {
            // return here
        }

        let first_step = path.first().unwrap();
        let mut step_stack: Vec<MevPathStep> = vec![];

        let mut debt = vec![];
        let mut asset = vec![];
        let mut in_token = input_token.clone();
        asset.push((first_step, in_token));
        for step in path {
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
            debt.push((step, debt_token.clone()));
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
            asset.push((step, asset_token));
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
