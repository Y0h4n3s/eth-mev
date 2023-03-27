//! ABIs
//!
//! Contract ABIs are refactored into their own module to gracefully deal with allowing missing docs on the abigen macro.
#![allow(missing_docs)]

use ethers::{prelude::*};

abigen!(Aggregator, "src/abi/Aggregator.json");
abigen!(FlashbotsCheckAndSend, "src/abi/FlashbotsCheckAndSend.json");


