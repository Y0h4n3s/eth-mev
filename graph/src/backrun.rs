use ethers::types::Transaction;
use crate::mev_path::MevPath;

pub struct Backrun {
    tx: Transaction,
    path: MevPath
}
