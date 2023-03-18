use ethers::types::{Eip1559TransactionRequest, Transaction, U256};
use crate::mev_path::MevPath;
use crate::mev_path::PathResult;
#[derive(Clone, Debug)]
pub struct Backrun {
    pub pending_tx: Transaction,
    pub tx: Eip1559TransactionRequest,
    pub path: MevPath,
    pub profit: U256,
    pub gas_cost: U256,
    pub block_number: u64,
    pub result: PathResult
}
