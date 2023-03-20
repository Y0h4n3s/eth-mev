#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]

pub mod backrun;
pub mod mev_path;
pub mod single_arb;
use crate::backrun::Backrun;
use crate::mev_path::MevPath;
use crate::single_arb::ArbPath;
use bb8_bolt::bolt_client::Params;
use bb8_bolt::{
    bb8::Pool as BPool,
    bolt_client::Metadata,
    bolt_proto::{version::*, Value},
    Manager,
};
use bolt_proto::value::{Node, Relationship};
use bolt_proto::Message;
use ethers::abi::{AbiEncode, ParamType, StateMutability, Token};
use ethers::prelude::LocalWallet;
use ethers::prelude::SignerMiddleware;
use ethers::signers::Signer;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::Address;
use ethers::types::{
    transaction::eip2930::AccessList, Block, BlockId, BlockNumber, Eip1559TransactionRequest,
    NameOrAddress, Transaction, TxHash, U256, U64,
};
use ethers::types::{Bytes, I256};
use ethers::utils::keccak256;
use ethers_flashbots::{BundleRequest, BundleTransaction, FlashbotsMiddleware};
use ethers_providers::{Http, Middleware, ProviderExt};
use garb_sync_eth::{
    EventSource, LiquidityProviderId, LiquidityProviders, PendingPoolUpdateEvent, Pool,
    PoolUpdateEvent,
};
use itertools::Itertools;
use neo4rs::{query, Graph as NGraph};
use once_cell::sync::Lazy;
use petgraph::algo::all_simple_paths;
use petgraph::prelude::{Graph, NodeIndex};
use petgraph::visit::{Bfs, Dfs};
use petgraph::{
    visit::{IntoNeighborsDirected, NodeCount},
    Outgoing, Undirected,
};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::iter::from_fn;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::Duration;
use tracing::{debug, error, info};
use url::Url;

static PRIVATE_KEY: Lazy<String> = Lazy::new(|| std::env::var("ETH_PRIVATE_KEY").unwrap());
static BUNDLE_SIGNER_PRIVATE_KEY: Lazy<String> =
    Lazy::new(|| std::env::var("ETH_BUNDLE_SIGNER_PRIVATE_KEY").unwrap());

static CONTRACT_ADDRESS: Lazy<Address> = Lazy::new(|| {
    Address::from_str(&std::env::var("ETH_CONTRACT_ADDRESS").unwrap_or_else(|_| {
        std::env::args()
            .nth(6)
            .unwrap_or("0xA46356ba716631d87Ab3081635F06136662ae3C0".to_string())
    }))
    .unwrap()
});

static NEO4J_USER: Lazy<String> =
    Lazy::new(|| std::env::var("ETH_NEO4J_USER").unwrap_or("neo4j".to_string()));
static NEO4J_PASS: Lazy<String> =
    Lazy::new(|| std::env::var("ETH_NEO4J_PASS").unwrap_or("neo4j".to_string()));
static NEO4J_URL: Lazy<String> =
    Lazy::new(|| std::env::var("ETH_NEO4J_URL").unwrap_or("127.0.0.1:7687".to_string()));

fn combinations<T>(v: &[T], k: usize) -> Vec<Vec<T>>
where
    T: Clone,
{
    if k == 0 {
        return vec![vec![]];
    }
    let mut result = vec![];
    for i in 0..v.len() {
        let mut rest = v.to_vec();
        rest.remove(i);
        for mut c in combinations(&rest, k - 1) {
            c.push(v[i].clone());
            result.push(c);
        }
    }
    result
}

#[derive(Clone)]
pub struct Order {
    pub size: u128,
    pub decimals: u64,
    pub route: Vec<Pool>,
    pub profit: f64,
}

pub struct GraphConfig {
    pub from_file: bool,
    pub save_only: bool,
}

pub static CHECKED_COIN: Lazy<String> = Lazy::new(|| {
    std::env::var("ETH_CHECKED_COIN")
        .unwrap_or("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".to_string())
});

pub static MAX_SIZE: Lazy<f64> = Lazy::new(|| {
    std::env::var("ETH_MAX_SIZE")
        .unwrap_or("0.1".to_string())
        .parse()
        .unwrap()
});

pub async fn start(
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    updated_q: kanal::AsyncReceiver<Box<dyn EventSource<Event = PoolUpdateEvent>>>,
    pending_updated_q: kanal::AsyncReceiver<Box<dyn EventSource<Event = PendingPoolUpdateEvent>>>,
    routes: Arc<RwLock<kanal::AsyncSender<Backrun>>>,
    single_routes: Arc<Mutex<kanal::Sender<Vec<ArbPath>>>>,
    used_oneshot: tokio::sync::oneshot::Sender<Vec<[Arc<RwLock<Pool>>; 2]>>,
    config: GraphConfig,
) -> anyhow::Result<()> {
    Graph::<String, Pool, Undirected>::new_undirected();
    let pr = pools.read().await.clone();
    let manager = Manager::new(
        &NEO4J_URL.clone(),
        None,
        [V4_4, V4_3, 0, 0],
        Metadata::from_iter(vec![
            ("user_agent", "bolt-client/X.Y.Z"),
            ("scheme", "basic"),
            ("principal", &NEO4J_USER),
            ("credentials", &NEO4J_PASS),
        ]),
    )
    .await?;
    // Create a connection pool. This should be shared across your application.
    let conn_pool = Arc::new(BPool::builder().build(manager).await?);

    // Fetch and use a connection from the pool
    let mut conn = conn_pool.get().await?;
    let res = conn
        .run(
            "MATCH (n)
DETACH DELETE n",
            None,
            None,
        )
        .await?;
    let pull_meta = Metadata::from_iter(vec![("n", 1)]);
    let (records, response) = conn.pull(Some(pull_meta)).await?;

    let mut locked_pools: HashMap<Pool, [Arc<RwLock<Pool>>; 2]> = HashMap::new();
    let cores = num_cpus::get();
    let permits = Arc::new(Semaphore::new(cores*2));
    let mut handles = vec![];
    for (_, pool) in pr.into_iter() {
        let permit = permits.clone().acquire_owned().await?;

        let mut pool_xy = pool.clone();
        let mut pool_yx = pool.clone();
        pool_xy.x_to_y = true;
        pool_yx.x_to_y = false;

        locked_pools.insert(
            pool.clone(),
            [
                Arc::new(RwLock::new(pool_xy)),
                Arc::new(RwLock::new(pool_yx)),
            ],
        );
        let con_pool = conn_pool.clone();

        handles.push(tokio::spawn(async move {
            let mut conn = con_pool.get().await.unwrap();

            let res = conn
            .run(
                    "MERGE (t:Token {address: $address}) RETURN t",
                Some(Params::from_iter(vec![("address", pool.x_address.clone())])),
                None,
            )
            .await.unwrap();
            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();

            let res = conn
            .run(
                    "MERGE (t:Token {address: $address}) RETURN t",
                Some(Params::from_iter(vec![("address", pool.y_address.clone())])),
                None,
            )
            .await.unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();

            let res = conn
            .run(
                    "MERGE (p:Pool {address: $address, x_to_y: $x_to_y, provider: $provider}) RETURN p",
                Some(Params::from_iter(vec![
                    ("address", pool.address.clone()),
                    ("x_to_y", pool.x_to_y.to_string()),
                    ("provider", serde_json::to_string(&pool.provider).unwrap()),
                ])),
                None,
            )
            .await.unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();

            let res = conn
            .run(
                    "MERGE (p:Pool {address: $address, x_to_y: $x_to_y, provider: $provider}) RETURN p",
                Some(Params::from_iter(vec![
                    ("address", pool.address.clone()),
                    ("x_to_y", (!pool.x_to_y).to_string()),
                    ("provider", serde_json::to_string(&pool.provider).unwrap()),
                ])),
                None,
            )
            .await.unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();

            let res = conn
            .run(
                    "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $y_address  MERGE (a)-[r:ASSET]->(b) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", true.to_string()),
                    ("y_address", pool.y_address.clone()),
                ])),
                None).await.unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();
            let res = conn
            .run(
                    "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $x_address  MERGE (b)-[r:DEBT]->(a) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", true.to_string()),
                    ("x_address", pool.x_address.clone()),
                ])),
                None).await.unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();

            let res = conn
            .run(
                    "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $x_address  MERGE (a)-[r:ASSET]->(b) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", false.to_string()),
                    ("x_address", pool.x_address.clone()),
                ])),
                None).await.unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();

            let res = conn
            .run(
                    "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $y_address  MERGE (b)-[r:DEBT]->(a) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", false.to_string()),
                    ("y_address", pool.y_address.clone()),
                ])),
                None).await.unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();
            drop(permit)
        }));

    }
    futures::future::join_all(handles).await;

    // txn.run_queries(queries).await.unwrap();
    // txn.commit().await.unwrap();

    let mut checked_coin_indices: Vec<NodeIndex> = vec![];

    let path_lookup = Arc::new(RwLock::new(
        HashMap::<Pool, HashSet<(String, Vec<Pool>)>>::new(),
    ));
    let path_lookup1 = Arc::new(RwLock::new(HashMap::<Pool, Vec<MevPath>>::new()));

    let max_intermidiate_nodes = 4;
    for i in 2..max_intermidiate_nodes {
        info!("Preparing {} step routes ", i);
        let path_lookup = path_lookup.clone();
        let pool = conn_pool.clone();
        let mut conn = pool.get().await?;
        let cores = num_cpus::get();
        let permits = Arc::new(Semaphore::new(cores*2));
        // OLD MATCHER: match cyclePath=(m1:Token{{address:'{}'}})-[*{}..{}]-(m2:Token{{address:'{}'}}) RETURN relationships(cyclePath) as cycle, nodes(cyclePath)
        let mut steps = "".to_string();
        let mut where_clause = "1 = 1".to_string();
        for j in 0..i {
            steps = steps + &format!("-[:DEBT]-(p{}:Pool)-[:ASSET]", j);
            if j != i - 1 {
                steps = steps + &format!("-(t{}:Token)", j);
                for k in 0..i - 1 {
                    if k == j {
                        continue;
                    }
                    where_clause += &format!(" AND t{}.address <> t{}.address", j, k)
                }
                for k in 0..i {
                    if k == j {
                        continue;
                    }
                    where_clause += &format!(" AND p{}.address <> p{}.address", j, k)
                }
                where_clause += &format!(" AND t.address <> t{}.address", j)
            }
        }
        let query = format!(
                "MATCH path=(t:Token{{address: '{}'}}){}->(t) WHERE {} return nodes(path);",
            CHECKED_COIN.clone(),
            steps,
            where_clause
        );
        debug!("{}", query);

        let res = conn.run(query, None, None).await?;
        let pull_meta = Metadata::from_iter(vec![("n", 50000)]);
        let (mut records, mut response) = conn.pull(Some(pull_meta.clone())).await?;
        loop {
            let mut handles = vec![];

            // populate path_lookup with this batch
            for record in records {
                let permit = permits.clone().acquire_owned().await?;
                let path_lookup1 = path_lookup1.clone();
                let pools = pools.clone();
                let locked_pools = locked_pools.clone();
                handles.push(tokio::spawn(async move {
                    let ps = pools.read().await.clone();
                    let mut pools =
                        record.fields().into_par_iter().filter_map(|val| {
                            match val.clone() {
                                Value::List(rels) => {

                                    Some(rels.iter().filter_map(|rel| {
                                        match rel {
                                            Value::Node(rel) => {
                                                if let Some(is_pool) = rel
                                                    .labels()
                                                    .iter()
                                                    .find(|p| *p == &"Pool".to_string())
                                                {
                                                    let address =
                                                        match rel.properties().get("address") {
                                                        Some(Value::String(s)) => s.clone(),
                                                            _ => "0x0".to_string(),
                                                        };
                                                    let provider =
                                                        match rel.properties().get("provider") {
                                                        Some(Value::String(provider)) => {
                                                            LiquidityProviders::from(provider)
                                                        }
                                                            _ => LiquidityProviders::UniswapV2(
                                                                    Default::default(),
                                                            ),
                                                        };
                                                    let x_to_y =
                                                        match rel.properties().get("x_to_y") {
                                                        Some(Value::String(x_to_y)) => {
                                                            if x_to_y == &"true".to_string() {
                                                                true
                                                            } else {
                                                                false
                                                            }
                                                        }
                                                            _ => false,
                                                        };
                                                    match ps.iter().find(|(_, p)| {
                                                        p.address == address
                                                        && p.provider == provider
                                                    }) {
                                                        Some((s, pool)) => {
                                                            let mut new = pool.clone();
                                                            new.x_to_y = x_to_y;
                                                            Some(new)
                                                        }
                                                        _ => None,
                                                    }
                                                } else {
                                                    None
                                                }
                                            }

                                            _ => None,
                                        }
                                    })
                                    .collect::<Vec<Pool>>())
                                }
                                _ => None

                            }
                        }).collect::<Vec<Vec<Pool>>>();
                    let paths = futures::future::join_all(pools.into_iter().map(|r| async {

                        let mut seen_count = 0;
                        r.iter().for_each(|p| {
                            if p.x_address == CHECKED_COIN.clone() {
                                seen_count = seen_count + 1;
                            }
                            if p.y_address == CHECKED_COIN.clone() {
                                seen_count = seen_count + 1;
                            }
                        });

                        if seen_count != 2 {
                            None
                        } else {
                            let mut locked = vec![];
                            for pool in &r {
                                let l_pools = locked_pools.get(pool).unwrap();
                                if pool.x_to_y {
                                    locked.push(l_pools[0].clone());
                                } else {
                                    locked.push(l_pools[1].clone());
                                }
                            }
                            if let Some(mut path) =
                                            MevPath::new(r, &locked, &CHECKED_COIN.clone()).await
                                        {
                                            if path.is_valid().await {
                                                Some(path)
                                            } else {
                                                None
                                            }
                                        } else {
                                None
                            }
                        }
                    })).await
                    .into_iter()
                    .filter_map(|p| p);


                    for p in paths {
                        let mut in_ = CHECKED_COIN.clone();

                        for pool in p.pools.iter() {
                            let mut w = path_lookup1.write().await;
                            if let Some(mut existing) = w.get_mut(&pool) {
                                existing.push(p.clone());
                            } else {
                                let mut set = vec![];
                                set.push(p.clone());
                                w.insert(pool.clone(), set);
                            }
                        }
                    }
                    drop(permit);
                }));
            }
            for handle in handles {
                handle.await?;
            }

            // query next batch from stream
            match &response {
                Message::Success(success) => {
                    if let Some(has_more) = success.metadata().get("has_more") {
                        match has_more {
                            Value::Boolean(val) => {
                                if *val {
                                    (records, response) =
                                        conn.pull(Some(pull_meta.clone())).await?;
                                } else {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }

        let mut total_paths = 0;
        for (_pool, paths) in path_lookup1.read().await.clone() {
            for path in paths {
                total_paths += path.path.len();
            }
        }
        info!("Done {} step {}", i, total_paths);
    }

    let mut total_paths = 0;
    for (_pool, paths) in path_lookup1.read().await.clone() {
        for path in paths {
            total_paths += path.path.len();
        }
    }
    info!("Found {} routes", total_paths);
    let mut uniq = path_lookup1
        .read()
        .await
        .clone()
        .iter()
        .map(|(pool, path)| {
            path.iter()
                .map(|p| p.pools.clone())
                .flatten()
                .collect::<Vec<Pool>>()
        })
        .flatten()
        .unique()
        .collect::<Vec<Pool>>();

    let uniq_locked = uniq
        .iter()
        .map(|pl| locked_pools.get(pl).unwrap())
        .cloned()
        .collect::<Vec<[Arc<RwLock<Pool>>; 2]>>();
    //	return Ok(());
    info!(
        "Registering Gas consumption for {} pool transactions",
        uniq.len()
    );
    let gas_map: Arc<std::sync::RwLock<HashMap<String, U256>>> =
        Arc::new(std::sync::RwLock::new(HashMap::new()));
    let node_url = "http://65.21.198.115:8545".to_string();
    let gas_lookup = gas_map.clone();

    let signer = PRIVATE_KEY.clone().parse::<LocalWallet>().unwrap();
    let signer_wallet_address = signer.address();
    let provider = ethers_providers::Provider::<Http>::connect(&node_url).await;
    let block = provider.get_block(BlockNumber::Latest).await.unwrap().unwrap();
    let latest_block = block.number.unwrap();
    let nonce = provider
        .get_transaction_count(signer_wallet_address, Some(BlockId::from(latest_block)))
        .await
        .unwrap();

    let mut client = Arc::new(FlashbotsMiddleware::new(
        provider,
        Url::parse("https://relay.flashbots.net").unwrap(),
        BUNDLE_SIGNER_PRIVATE_KEY
            .clone()
            .parse::<LocalWallet>()
            .unwrap(),
    ));

    let mut join_handles = vec![];
    for pool in uniq.clone() {
        let signer = signer.clone();
        let client = client.clone();
        let gas_lookup = gas_lookup.clone();
        join_handles.push(tokio::runtime::Handle::current().spawn(async move {
            let client = client.clone();
            let mut ix_data = "".to_string();
            let packed_asset = MevPath::encode_packed(I256::from(1));

            let function = match pool.provider.id() {
                LiquidityProviderId::UniswapV2
                | LiquidityProviderId::SushiSwap
                | LiquidityProviderId::Solidly
                | LiquidityProviderId::Pancakeswap => {
                    ix_data = "0e000000".to_string()
                        + if pool.x_to_y { "01" } else { "00" }
                        + &pool.address[2..]
                        + &(packed_asset.len() as u8).encode_hex()[64..]
                        + &packed_asset;
                }
                LiquidityProviderId::UniswapV3 => {
                    ix_data = "00000600".to_string()
                        + if pool.x_to_y { "01" } else { "00" }
                        + &pool.address[2..]
                        + &(packed_asset.len() as u8).encode_hex()[64..]
                        + &packed_asset;
                }
                LiquidityProviderId::BalancerWeighted => {
                    let mut w = gas_lookup.write().unwrap();
                    w.insert(pool.address.clone(), U256::from(120000));
                    return;
                }
            };

            let tx_request = Eip1559TransactionRequest {
                to: Some(NameOrAddress::Address(CONTRACT_ADDRESS.clone())),
                from: None,
                data: Some(ethers::types::Bytes::from_str(&ix_data).unwrap()),
                chain_id: Some(U64::from(1)),
                max_priority_fee_per_gas: None,
                // update later
                max_fee_per_gas: block.base_fee_per_gas,
                gas: Some(U256::from(500000)),
                nonce: Some(nonce),
                value: None,
                access_list: AccessList::default(),
            };

            let typed_tx = TypedTransaction::Eip1559(tx_request.clone());
            let tx_sig = signer.sign_transaction(&typed_tx).await.unwrap();
            let signed_tx = typed_tx.rlp_signed(&tx_sig);
            let mut bundle = BundleRequest::new();
            bundle = bundle
                .push_transaction(signed_tx)
                .set_block(latest_block)
                .set_simulation_block(latest_block)
                .set_simulation_timestamp(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                );

            let simulation_result = client.simulate_bundle(&bundle).await;

            if let Ok(res) = simulation_result {
                let tx = res.transactions.get(0).unwrap();
                let gas_used = tx.gas_used;
                let mut w = gas_lookup.write().unwrap();
                if tx.error.is_none() && tx.revert.is_none() {
                    w.insert(pool.address.clone(), gas_used + U256::from(5000));
                } else {
                    w.insert(pool.address.clone(), gas_used + U256::from(20000));
                }
                debug!("{} uses {:?}", pool.address, gas_used + U256::from(300))
            } else {
                error!(
                    "Failed to estimate gas for {} {:?}",
                    pool.address,
                    simulation_result.unwrap_err()
                )
            }
        }));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    for task in join_handles {
        task.await.unwrap();
    }

    info!("Starting Listener thread");
    info!(
        "Clearing {} cached events",
        updated_q.len() + pending_updated_q.len()
    );

    let mut workers = vec![];
    let cores = num_cpus::get();

    let gas_lookup = gas_map.clone();
    for i in 0..cores / 2 {
        let gas_lookup = gas_lookup.clone();
        let path_lookup = path_lookup1.clone();
        let single_routes = single_routes.clone();
        let updated_q = updated_q.clone();
        workers.push(tokio::spawn(async move {
            while let Ok(updated_market_event) = updated_q.recv().await {
                let event = updated_market_event.get_event();
                let mut updated_market = event.pool;
                let market_routes =
                    if let Some(market_routes) = path_lookup.read().await.get(&updated_market) {
                        market_routes.clone()
                    } else {
                        debug!("No routes found for {}", updated_market);
                        continue;
                    };
                let single_routes = single_routes.clone();
                let gas_lookup = gas_lookup.clone();

                tokio::spawn(async move {
                    let mut updated = vec![];
                    for route in market_routes {
                        if let Some((tx, result)) = route.get_transaction().await {
                            let r = gas_lookup.read().unwrap();
                            let mut gas_cost = U256::zero();
                            for pool in &route.pools {
                                gas_cost += *r.get(&pool.address).unwrap_or(&U256::from(100000));
                            }
                            drop(r);
                            updated.push(ArbPath {
                                path: route,
                                tx,
                                profit: U256::from(result.profit),
                                gas_cost,
                                block_number: event.block_number,
                                result,
                            })
                        }
                    }
                    let mut w = single_routes.lock().unwrap();
                    w.send(updated).unwrap();
                });
            }
        }));
    }

    let gas_lookup = gas_map.clone();
    for i in 0..cores / 2 {
        let gas_lookup = gas_lookup.read().unwrap().clone();

        let path_lookup = path_lookup1.clone();
        let routes = routes.read().await.clone();
        let pending_updated_q = pending_updated_q.clone();
        workers.push(tokio::spawn(async move {
            while let Ok(updated_market_event) = pending_updated_q.recv().await {
                let event = updated_market_event.get_event();
                let mut updated_market = event.pool;
                let routes = routes.clone();
                let market_routes =
                    if let Some(market_routes) = path_lookup.read().await.get(&updated_market) {
                        market_routes.clone()
                    } else {
                        debug!("No routes found for pending update {}", updated_market);
                        continue;
                    };
                let mut updated = market_routes
                    .into_par_iter()
                    .filter_map(|mut route| {
                        route.get_backrun_for_update(
                            event.pending_tx.clone(),
                            updated_market.clone(),
                            &gas_lookup,
                            event.block_number,
                        )
                    })
                    .collect::<Vec<Backrun>>();

                for opportunity in updated {
                    routes.send(opportunity).await;
                }
            }
        }));
    }

    let mut watch_pools: HashMap<String, Pool> = HashMap::new();
    uniq.iter().for_each(|pool| {
        watch_pools.insert(pool.address.clone(), pool.clone());
    });
    used_oneshot.send(uniq_locked).unwrap();
    for worker in workers {
        worker.await.unwrap();
    }

    Ok(())
}

pub fn decimals(coin: String) -> u64 {
    match coin.as_str() {
        "0x1::aptos_coin::AptosCoin" => 18,
        _ => 18,
    }
}
