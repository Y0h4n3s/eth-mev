#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]
#![allow(deprecated)]

// TODO: node url dispacher
mod abi;

use tracing::{debug, error, info, warn};
//Deployer: 0xef344B9eFcc133EB4e7FEfbd73a613E3b2D05e86
// Deployed to: 0x5F416E55fdBbA8CC0D385907C534B57a08710c35
// Transaction hash: 0x2f03f71cc6915fcc17afd71730f1fb6809cef88b102fcb3aa3e3dc60095d1aee
//V2
//	Deployer: 0xef344B9eFcc133EB4e7FEfbd73a613E3b2D05e86
// Deployed to: 0x3fDaA7c06379981c879F7a9617470215f808368F
// Transaction hash: 0x1772c078cc5749dd0dcbebf316280ac06eb41735d501abd8a2a13521db6e16bd
use once_cell::sync::Lazy;

use tokio::runtime::Runtime;
use async_std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::{HashMap};
use ethers::prelude::{Address, H160, LocalWallet, SignerMiddleware};
use ethers::prelude::StreamExt;

use url::Url;
use garb_sync_eth::{EventSource, LiquidityProviders, PendingPoolUpdateEvent, Pool, PoolUpdateEvent, SyncConfig};
use std::str::FromStr;
use ethers::abi::{AbiEncode, ParamType, StateMutability, Token};
use ethers::core::k256::elliptic_curve::consts::U25;
use ethers::prelude::k256::elliptic_curve::consts::U2;

use ethers::signers::Signer;
use ethers::types::{U256, Block, TxHash, BlockId, Eip1559TransactionRequest, BlockNumber, U64, NameOrAddress, transaction::eip2930::AccessList};
use ethers::types::transaction::eip2718::TypedTransaction;
use tokio::time::Duration;
use ethers_providers::{Http, Middleware};
use ethers_flashbots::{BundleRequest, FlashbotsMiddleware, BundleTransaction};
use garb_graph_eth::{GraphConfig, Order};
use rand::Rng;


static PROVIDERS: Lazy<Vec<LiquidityProviders>> = Lazy::new(|| {
    std::env::var("ETH_PROVIDERS").unwrap_or_else(|_| std::env::args().nth(5).unwrap_or("1".to_string()))
        .split(",")
        .map(|i| LiquidityProviders::from(i))
        .collect()
});

static CONTRACT_ADDRESS: Lazy<Address> = Lazy::new(|| {
    Address::from_str(&std::env::var("ETH_CONTRACT_ADDRESS").unwrap_or_else(|_| std::env::args().nth(6).unwrap_or("0x3fDaA7c06379981c879F7a9617470215f808368F".to_string()))).unwrap()
});

static NODE_URL: Lazy<Url> = Lazy::new(|| {
    let url = std::env::var("ETH_NODE_URL").unwrap_or_else(|_| std::env::args().nth(7).unwrap_or("http://89.58.31.215:8545".to_string()));
    Url::parse(&url).unwrap()
});

static PRIVATE_KEY: Lazy<String> = Lazy::new(|| std::env::var("ETH_PRIVATE_KEY").unwrap());
static BUNDLE_SIGNER_PRIVATE_KEY: Lazy<String> = Lazy::new(|| std::env::var("ETH_BUNDLE_SIGNER_PRIVATE_KEY").unwrap());

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(tracing::Level::INFO)
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    let rt = Runtime::new()?;
    rt.block_on(async_main())?;
    Ok(())
}

pub async fn async_main() -> anyhow::Result<()> {
    let pools = Arc::new(RwLock::new(HashMap::<String, Pool>::new()));
    let (update_q_sender, update_q_receiver) = kanal::bounded_async::<Box<dyn EventSource<Event=PoolUpdateEvent>>>(1000);
    let (pending_update_q_sender, pending_update_q_receiver) = kanal::bounded_async::<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>(1000);
    // routes holds the routes that pass through an updated pool
    // this will be populated by the graph module when there is an updated pool
    let (routes_sender, mut routes_receiver) =
        kanal::bounded_async::<Vec<Eip1559TransactionRequest>>(10000);

    let sync_config = SyncConfig {
        providers: PROVIDERS.clone(),
    };
    garb_sync_eth::start(pools.clone(), update_q_sender, pending_update_q_sender, sync_config)
        .await
        .unwrap();

    let mut joins = vec![];

    let graph_conifg = GraphConfig {
        from_file: false,
        save_only: false,
    };
    let graph_routes = routes_sender.clone();
    joins.push(std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();

        rt.block_on(async move {
            garb_graph_eth::start(pools.clone(), update_q_receiver, pending_update_q_receiver,Arc::new(RwLock::new(graph_routes)), graph_conifg).await.unwrap();
        });
    }));
    joins.push(std::thread::spawn(move || {
        transactor(&mut routes_receiver, routes_sender).unwrap();
    }));

    for join in joins {
        join.join().unwrap()
    }
    Ok(())
}

pub fn calculate_next_block_base_fee(block: Block<TxHash>) -> anyhow::Result<U256> {
    // Get the block base fee per gas
    let base_fee = block
        .base_fee_per_gas.unwrap();

    // Get the mount of gas used in the block
    let gas_used = block.gas_used;

    // Get the target gas used
    let mut target_gas_used = block.gas_limit / 2;
    target_gas_used = if target_gas_used == U256::zero() {
        U256::one()
    } else {
        target_gas_used
    };

    // Calculate the new base fee
    let new_base_fee = {
        if gas_used > target_gas_used {
            base_fee
                + ((base_fee * (gas_used - target_gas_used)) / target_gas_used) / U256::from(8u64)
        } else {
            base_fee
                - ((base_fee * (target_gas_used - gas_used)) / target_gas_used) / U256::from(8u64)
        }
    };

    // Add a random seed so it hashes differently
    let seed = rand::thread_rng().gen_range(0..9);
    Ok(new_base_fee + seed)
}



pub fn transactor(routes: &mut kanal::AsyncReceiver<Vec<Eip1559TransactionRequest>>, _routes_sender: kanal::AsyncSender<Vec<Eip1559TransactionRequest>>) -> anyhow::Result<()> {
    let mut workers = vec![];
    let cores = num_cpus::get();

    for i in 0..cores {
        let routes = routes.clone();
        workers.push(std::thread::spawn(move || {
            let rt = Runtime::new().unwrap();

            rt.block_on(async move {
                let signer = PRIVATE_KEY.clone().parse::<LocalWallet>().unwrap();

                let bundle_signer = BUNDLE_SIGNER_PRIVATE_KEY.clone().parse::<LocalWallet>().unwrap();
                let provider = ethers_providers::Provider::<Http>::try_from(NODE_URL.clone().to_string()).unwrap();
                let client = Arc::new(
                    SignerMiddleware::new_with_provider_chain(provider.clone(), signer.clone()).await.unwrap());


                let nonce = Arc::new(tokio::sync::RwLock::new(U256::from(0)));
                let block = Arc::new(tokio::sync::RwLock::new(Block::default()));
                let mut join_handles = vec![];

                let signer_wallet_address = signer.address();
                let nonce_update = nonce.clone();
                let ap = client.clone();

                join_handles.push(tokio::runtime::Handle::current().spawn(async move {
                    // keep updating nonce
                    loop {
                        if let Ok(n) = ap.get_transaction_count(signer_wallet_address, None).await {
                            let mut w = nonce_update.write().await;
                            *w = n;
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }));
                let block_update = block.clone();
                let ap = client.clone();

                join_handles.push(tokio::runtime::Handle::current().spawn(async move {
                    // keep updating nonce
                    loop {
                        if let Ok(Some(b)) = ap.get_block(BlockId::Number(BlockNumber::Latest)).await {
                            let mut w = block_update.write().await;
                            *w = b;
                        } else {
                            println!("transactor > Error getting block number", );
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }));

                let flashbots_client = Arc::new(SignerMiddleware::new(
                    FlashbotsMiddleware::new(
                        provider.clone(),
                        Url::parse("https://relay.flashbots.net").unwrap(),
                        bundle_signer.clone(),
                    ),
                    signer.clone(),
                ));
                let signer = Arc::new(signer);


                let sent = Arc::new(RwLock::new(false));
                let signer = Arc::new(signer);
                while let Ok(orders) = routes.recv().await {
                    info!("{}. Received {}",i, orders.len());
                    let mut handles = vec![];
                    for order in orders {
                        let nonce_num = nonce.clone();
                        let block = block.clone();
                        let signer = signer.clone();
                        let client = client.clone();
                        let flashbots_client = flashbots_client.clone();
                        handles.push(tokio::runtime::Handle::current().spawn(async move {
                            let mut tx_request = order;
                            tx_request.to = Some(NameOrAddress::Address(CONTRACT_ADDRESS.clone()));
                            tx_request.from = Some(signer_wallet_address);
                            let n = nonce_num.read().await;
                            let blk = block.read().await;
                            let base_fee = blk.base_fee_per_gas.unwrap();
                            tx_request.max_fee_per_gas = Some(tx_request.max_priority_fee_per_gas.unwrap().max(base_fee).min(base_fee.checked_mul(U256::from(2)).unwrap()));
                            tx_request.max_priority_fee_per_gas = Some(base_fee);
                            tx_request.nonce = Some(n.clone().checked_add(U256::from(0)).unwrap());
                            drop(n);
                            drop(blk);

                            // profit doesn't cover tx_fees
                            if tx_request.max_fee_per_gas.unwrap() == base_fee {
                                tx_request.max_fee_per_gas = Some(base_fee.checked_mul(U256::from(2)).unwrap());
                            }
                            let blk = tx_request.value.unwrap().as_u64();
                            tx_request.value = None;
                            //
                            // // gas * max_priority_fee_per_gas / 10 ^ 8 <= profit amount
                            // let tx_request = Eip1559TransactionRequest {
                            //     to: Some(NameOrAddress::Address(CONTRACT_ADDRESS.clone())),
                            //     from: Some(signer_wallet_address),
                            //     data: Some(ethers::types::Bytes::from(tx_data)),
                            //     chain_id: Some(U64::from(1)),
                            //     max_priority_fee_per_gas: Some(U256::from(0)),
                            //     max_fee_per_gas: Some(blk.base_fee_per_gas.unwrap().checked_add(blk.base_fee_per_gas.unwrap().checked_div(U256::from(2)).unwrap()).unwrap_or(U256::from(0))),
                            //     gas: Some(U256::from(1000000)),
                            //     nonce: Some(n.clone().checked_add(U256::from(0)).unwrap()),
                            //     value: None,
                            //     access_list: AccessList::default(),
                            // };
                            // drop(n);
                            //

                            let typed_tx = TypedTransaction::Eip1559(tx_request.clone());
                            let tx_sig = signer.sign_transaction(&typed_tx).await.unwrap();
                            let signed_tx = typed_tx.rlp_signed(&tx_sig);
                            let mut bundle_request = BundleRequest::new();
                            let bundled: BundleTransaction = signed_tx.clone().into();

                            bundle_request = bundle_request.push_transaction(bundled).set_block(U64::from(blk)).set_simulation_block(U64::from(blk)).set_simulation_timestamp(0);
                            // drop(blk);
                            // let mut sent_val = sent_v.write().await;
                            // if !(*sent_val) {
                            //
                            //     *sent_val = false;
                            //     drop(sent_val);
                            //
                            //     // let result = client.send_transaction(typed_tx, Some(BlockId::Number(BlockNumber::Number(U64::from(blk))))).await;
                            //     // println!("{:?}", result.unwrap());
                            info!("{}. ->  {} {:?} {:?} {:?}",i+1,tx_request.gas.unwrap(), blk, tx_request.max_priority_fee_per_gas.unwrap(), tx_request.max_fee_per_gas.unwrap());

                            // let simulated_bundle = flashbots_client.inner().simulate_bundle(&bundle_request).await;
                            //
                            // match simulated_bundle {
                            //     Ok(res) => {
                            //         let ex_tx = res.transactions.get(0).unwrap();
                            //         //                                 if ex_tx.gas_used < U256::from(270000) {
                            //         // let result = client.send_transaction(typed_tx, Some(BlockId::Number(BlockNumber::Number(U64::from(blk))))).await;
                            //         //                                         println!("{:?}", result.unwrap());
                            //         //                                 }
                            //         if ex_tx.error.is_none() {
                            //             println!("\n\n\n\n\n{:?}\n\n\n\n\n\n", ex_tx);
                            //         } else {
                            //             println!("{:?} {:?}", ex_tx.gas_used, ex_tx.revert)
                            //         }
                            //     }
                            //     Err(err) => {
                            //         error!("{:?}", err)
                            //     }
                            // }

                            //
                            //                   }
                            let real_bundle = flashbots_client.inner().send_bundle(&bundle_request).await.unwrap().await;
                            println!("{:?}", real_bundle);

                            // match simulated_bundle {
                            //     Ok(bundle) => {
                            //
                            //     }
                            //     Err(e) =>
                            //         println!("{:?}", e)
                            // }
                        }));
                    }
                    futures::future::join_all(handles).await;

                }
                for task in join_handles {
                    task.await.unwrap();
                }
            })
        }));
    }
    for worker in workers {
        worker.join();
    }


    Ok(())
}

fn hex_to_address_string(hex: String) -> String {
    ("0x".to_string() + hex.split_at(26).1).to_string()
}