use backtrack::problem::{Check, Scope};
use backtrack::solvers::{ IterSolveCached};
use ethers::types::Eip1559TransactionRequest;
use garb_sync_eth::{Pool, PoolInfo};
// helper trait to filter solutions of interest
use backtrack::solve::IterSolveExt;
use itertools::Itertools;


const MINIMUM_PATH_LENGTH: usize = 2;
pub struct ExpandedPath {
    pub path: Vec<Pool>,
    pub size: usize,
}

impl ExpandedPath {
    pub fn new(pools: &Vec<Pool>) -> Self {
        Self {
            path: pools
                .iter()
                .cloned()
                .map(|pool| {
                    let mut other = pool.clone();
                    other.x_to_y = !other.x_to_y;
                    vec![other, pool]
                })
                .flatten()
                .collect(),
            size: pools.len(),
        }
    }
}

impl Scope<'_, Pool> for ExpandedPath {
    fn len(&self) -> usize {
        self.path.len()
    }
    fn size(&self) -> usize {
        self.size
    }
    fn value(&'_ self, index: usize) -> Pool {
        self.path.get(index).unwrap().clone()
    }

}

impl Check<Pool> for ExpandedPath {
    fn extends_sat(&self, solution: &[Pool], x: &Pool) -> bool {

        if solution.iter().find(|p| p.eq(&x)).is_some() {
            return false;
        }

        if solution.len() < self.size - 1  {
            return true;
        }


        let mut debt = vec![];
        let mut asset = vec![];
        let mut proposed = Vec::from(solution);
        proposed.push(x.clone());

        for pool in proposed.iter() {
            let (asset_token, debt_token) = if pool.x_to_y {
                (&pool.y_address, &pool.x_address)
            } else {
                (&pool.x_address, &pool.y_address)
            };
            debt.push((&pool.address, debt_token));
            if let Some((index, _)) = debt
                .iter()
                .find_position(|(p, t)| t == &asset_token && *p != &pool.address)
            {
                debt.remove(index);
            } else {
                asset.push((&pool.address, asset_token));
            }
        }

        for (pool, asset_token) in asset {
            if let Some((index, _)) = debt.iter().find_position(|(_p, t)| t == &asset_token) {
                debt.remove(index);
            }
        }
//        if debt.len() == 0 && proposed.len() == self.size {
//            println!("`````````````````````` Tried Route ``````````````````````");
//            for (i, pool) in proposed.iter().enumerate() {
//                println!("{}. {}", i + 1, pool);
//            }
//        }
        return debt.len() == 0 && proposed.len() == self.size;
    }
}

#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct MevPath {
    pub paths: Vec<Vec<MevPathStep>>,
    pub arrangements: Vec<Vec<Pool>>,
    pub input_token: String
}
#[derive(Hash, PartialEq, Eq, Debug, Clone)]

pub enum MevPathStep {
    ExactIn(Pool),
    ExactOut(Pool),
    // pool: the pool to pay back
    // bool: the token we're paying back, true for x false for y
    Payback(Pool, bool)
}


pub struct MevPathUpdateResult {
    // sorted by highest output transactions
    pub transactions: Vec<Eip1559TransactionRequest>,
}

impl MevPath {
    pub fn new(pools: &Vec<Pool>, input_token: &String) -> Self {
        let expanded = ExpandedPath::new(pools);
        let solver = IterSolveCached::new(&expanded);
        let mut sats = solver.sat_iter();
        let mut arrangements = vec![];
        let mut paths = vec![];
        while let Some(nxt) = sats.next() {
//            println!("`````````````````````` Tried Route ``````````````````````");
//                        for (i, pool) in nxt.iter().enumerate() {
//                            println!("{}. {}", i + 1, pool);
//                        }
            arrangements.push(nxt);
            if let Ok(path) = Self::process_path(nxt, input_token) {
                paths.push(path)
            }
        }
        Self {
            arrangements,
            input_token: input_token.clone(),
            paths
        }
    }
    pub fn update(updated_pool: Pool) -> MevPathUpdateResult {
        MevPathUpdateResult { transactions: vec![] }
    }

    pub fn process_path(path: Vec<Pool>, input_token: &String) -> anyhow::Result<Vec<MevPathStep>> {
        if path.len() < MINIMUM_PATH_LENGTH {
            // return here
        }

        let first_step = path.first().unwrap();
        let mut step_stack: Vec<MevPathStep> = vec![];
        let start_exact_in = if first_step.x_to_y {
            if input_token == &first_step.x_address {
                Ok(true)
            } else {
                if !first_step.supports_callback_payment() {
                    return Err(anyhow::Error::msg("Pool Doesn't support callback payment"))
                }
                Ok(false)

            }
        }
        else  {
            if input_token == &first_step.x_address {
                if !first_step.supports_callback_payment() {
                    return Err(anyhow::Error::msg("Pool Doesn't support callback payment"))
                }
                Ok(false)
            } else {
                Ok(true)

            }
        };

        if let Ok(start_exact_in) = start_exact_in {
            if start_exact_in {
                let mut debt = vec![];
                let mut asset = vec![];
                for step in path {
                    let (asset_token, debt_token) = if step.x_to_y {
                        (&step.y_address, &step.x_address)
                    } else {
                        (&step.x_address, &step.y_address)
                    };
                    debt.push(debt_token);
                    if step_stack.len() == 0 {
                        step_stack.push(
                                MevPathStep::ExactIn(step.clone())
                                );
                        asset.push(asset_token);
                        continue
                    }

                    let last_step = step_stack.last().unwrap();
                    
                }
            }
        } else {
            return start_exact_in.unwrap_err()
        }
        Ok(())
    }
}

// keep this logic seprate
struct TransactionBuilder {

}

