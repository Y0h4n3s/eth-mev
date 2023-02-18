use backtrack::problem::{Check, Scope};
use backtrack::solvers::IterSolveNaive;
use ethers::types::Eip1559TransactionRequest;
use garb_sync_eth::Pool;
// helper trait to filter solutions of interest
use backtrack::solve::IterSolveExt;
use itertools::Itertools;

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
    fn is_empty(&self) -> bool {
        false
    }
}

impl Check<Pool> for ExpandedPath {
    fn extends_sat(&self, solution: &[Pool], x: &Pool) -> bool {

        if solution.iter().find(|p| p.eq(&x) && x.x_to_y == p.x_to_y).is_some() {
            return false;
        }

        if solution.len() < self.size - 1  {
            return true;
        }


        let mut debt = vec![];
        let mut asset = vec![];
        let mut proposed = Vec::from(solution);
        proposed.push(x.clone());

        for pool in solution {
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
            if let Some((index, _)) = debt.iter().find_position(|(p, t)| t == &asset_token) {
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
    pub path: Vec<Pool>,
    pub arrangements: Vec<Vec<Pool>>
}

pub struct MevPathUpdateResult {
    // sorted by highest output transactions
    pub transactions: Vec<Eip1559TransactionRequest>,
}

impl MevPath {
    pub fn new(pools: Vec<Pool>) -> Self {
        let expanded = ExpandedPath::new(&pools);
        let solver = IterSolveNaive::new(&expanded);
        let mut sats = solver.sat_iter();
        let mut arrangements = vec![];
        while let Some(nxt) = sats.next() {
            arrangements.push(nxt);
        }
        Self {
            path: pools,
            arrangements
        }
    }
    pub fn update(updated_pool: Pool) {}
}
