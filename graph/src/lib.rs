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
use futures::stream::{FuturesUnordered, StreamExt, TryStreamExt};
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
use rand::Rng;
use rayon::prelude::*;
use rmp_serde::encode::{to_vec, write};
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
use tracing::{debug, error, info, warn};
use url::Url;
static IPC_PATH: Lazy<String> = Lazy::new(|| {
    std::env::var("ETH_IPC_PATH").unwrap_or_else(|_| {
        std::env::args()
            .nth(6)
            .unwrap_or("/root/.ethereum/geth.ipc".to_string())
    })
});
static PRIVATE_KEY: Lazy<String> = Lazy::new(|| std::env::var("ETH_PRIVATE_KEY").unwrap());
static BUNDLE_SIGNER_PRIVATE_KEY: Lazy<String> =
    Lazy::new(|| std::env::var("ETH_BUNDLE_SIGNER_PRIVATE_KEY").unwrap());

static CONTRACT_ADDRESS: Lazy<Address> = Lazy::new(|| {
    Address::from_str(&std::env::var("ETH_CONTRACT_ADDRESS").unwrap_or_else(|_| {
        std::env::args()
            .nth(6)
            .unwrap_or("0x856cd40Ce7ee834041A6Ea96587eA76200624517".to_string())
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
    single_routes: Arc<Mutex<kanal::Sender<Vec<ArbPath>>>>,
    used_oneshot: tokio::sync::oneshot::Sender<Vec<[Arc<RwLock<Pool>>; 2]>>,
    config: GraphConfig,
) -> anyhow::Result<()> {
    Graph::<String, Pool, Undirected>::new_undirected();
    let pr = pools.read().await.clone();
    let path_lookup1 = Arc::new(RwLock::new(HashMap::<Pool, Vec<MevPath>>::new()));
    let mut locked_pools: HashMap<Pool, [Arc<RwLock<Pool>>; 2]> = HashMap::new();

    if !config.from_file {
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
        let cores = num_cpus::get();

        // Create a connection pool. This should be shared across your application.
        let conn_pool = Arc::new(
            BPool::builder()
                .max_size(cores as u32 + 1)
                .build(manager)
                .await?,
        );

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

        for (_, pool) in pr.into_iter() {
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

            let res = conn
                .run(
                    "MERGE (t:Token {address: $address}) RETURN t",
                    Some(Params::from_iter(vec![("address", pool.x_address.clone())])),
                    None,
                )
                .await
                .unwrap();
            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();
            match response {
                Message::Failure(reason) => info!("IgnoredX: {}", pool.address),
                _ => (),
            }
            let res = conn
                .run(
                    "MERGE (t:Token {address: $address}) RETURN t",
                    Some(Params::from_iter(vec![("address", pool.y_address.clone())])),
                    None,
                )
                .await
                .unwrap();

            let pull_meta = Metadata::from_iter(vec![("n", -1)]);
            let (records, response) = conn.pull(Some(pull_meta)).await.unwrap();
            match response {
                Message::Failure(reason) => info!("IgnoredY: {}", pool.address),
                _ => (),
            }
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
            match response {
                Message::Failure(reason) => info!("IgnoredXY: {}", pool.address),
                _ => (),
            }
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
            match response {
                Message::Failure(reason) => info!("IgnoredYX: {}", pool.address),
                _ => (),
            }
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
            match response {
                Message::Failure(reason) => info!("IgnoredAXY:  {} {:?}", pool.address, reason),
                _ => (),
            }
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
            match response {
                Message::Failure(reason) => info!("IgnoredAYX:  {} {:?}", pool.address, reason),
                _ => (),
            }
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
            match response {
                Message::Failure(reason) => info!("IgnoredDXY:  {} {:?}", pool.address, reason),
                _ => (),
            }
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
            match response {
                Message::Failure(reason) => info!("IgnoredDYX: {} {:?}", pool.address, reason),
                _ => (),
            }
        }

        // txn.run_queries(queries).await.unwrap();
        // txn.commit().await.unwrap();

        let mut checked_coin_indices: Vec<NodeIndex> = vec![];

        let path_lookup = Arc::new(RwLock::new(
            HashMap::<Pool, HashSet<(String, Vec<Pool>)>>::new(),
        )); //140 311 10869 12059

        let max_intermidiate_nodes = 5;
        for i in 2..max_intermidiate_nodes {
            info!("Preparing {} step routes ", i);
            let path_lookup = path_lookup.clone();
            let pool = conn_pool.clone();
            let mut conn = pool.get().await?;
            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(cores * 2));
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
            info!("{}", query);

            let res = conn.run(query, None, None).await?;
            let pull_meta = Metadata::from_iter(vec![("n", 10000)]);
            let (mut records, mut response) = conn.pull(Some(pull_meta.clone())).await?;
            loop {
                let mut handles = vec![];
                let permit = permits.clone().acquire_owned().await?;
                let path_lookup1 = path_lookup1.clone();
                let pools = pools.clone();
                let locked_pools = locked_pools.clone();
                handles.push(tokio::spawn(async move {
                    let ps = pools.read().await.clone();

                    let paths = records.into_par_iter().map(|record| {
                        let record = record.clone();
                        record
                            .fields()
                            .to_vec()
                            .into_iter()
                            .filter_map(|val| {
                                let r = match val.clone() {
                                    Value::List(rels) =>
                                            rels.iter()
                                        .filter_map(|rel| match rel {
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
                                        })
                                        .collect::<Vec<Pool>>(),
                                    _ => vec![],
                                };
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
                                    let mut pools = r;
                                    pools.reverse();
                                    let mut locked = vec![];
                                    for pool in &pools {
                                        let l_pools = locked_pools.get(pool).unwrap();
                                        if pool.x_to_y {
                                            locked.push(l_pools[0].clone());
                                        } else {
                                            locked.push(l_pools[1].clone());
                                        }
                                    }
                                    let mut path =
                                        MevPath::new(pools, &locked, &CHECKED_COIN.clone());

                                        if let Ok(kind) = path.process_path(path.pools.clone(), &path.input_token) {
                                            path.optimal_path = kind;
                                            Some(path)
                                        } else {
                                            None
                                        }

                                }
                            }).collect_vec()



                    }).flatten().collect::<Vec<MevPath>>();
                    for p in paths {
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
                total_paths += paths.len();
            }
            info!("Done {} step {}", i, total_paths);
        }

        if config.save_only {
            let mut saved = HashMap::<Pool, Vec<Vec<Pool>>>::new();
            for (pool, paths) in path_lookup1.read().await.clone() {
                for path in paths {
                    if let Some(mut exsisting) = saved.get_mut(&pool) {
                        exsisting.push(path.pools)
                    } else {
                        let v = vec![path.pools];
                        saved.insert(pool.clone(), v);
                    }
                }
            }
            let encoded = to_vec(&saved).unwrap();
            let file = std::fs::File::create("paths.json").unwrap();
            let mut writer = std::io::BufWriter::new(file);
            rmp_serde::encode::write(&mut writer, &saved).unwrap();
            return Ok(());
        }
    } else {
        let file = std::fs::File::open("paths.json").unwrap();
        let mut reader = std::io::BufReader::new(file);
        let saved: HashMap<Pool, Vec<Vec<Pool>>> = rmp_serde::decode::from_read(reader).unwrap();
        let unique_pools = saved
            .clone()
            .into_iter()
            .map(|(pool, path)| path.into_iter().flatten().collect::<Vec<Pool>>())
            .flatten()
            .unique()
            .collect::<Vec<Pool>>();

        for pool in unique_pools {
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

            let paths = saved.get(&pool).unwrap();
        }
        let filter_tokens: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string("blacklisted_tokens.json").unwrap()).unwrap();
        let filter_pools: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string("blacklisted_pools.json").unwrap()).unwrap();
        let mut lookup: std::sync::Mutex<HashMap<Pool, Vec<MevPath>>> = std::sync::Mutex::new(HashMap::new());

        saved.into_par_iter().for_each(|(pool, paths)|{
            paths.into_par_iter().for_each(|r| {
                let mut locked = vec![];
                for pool in &r {
                    if filter_tokens.contains(&pool.x_address) || filter_tokens.contains(&pool.y_address) {
                        return
                    }
                    if filter_pools.contains(&pool.address) {
                        return
                    }
                    let l_pools = locked_pools.get(pool).unwrap();
                    if pool.x_to_y {
                        locked.push(l_pools[0].clone());
                    } else {
                        locked.push(l_pools[1].clone());
                    }
                }
                let mut path = MevPath::new(r, &locked, &CHECKED_COIN.clone());
                    if let Ok(kind) = path.process_path(path.pools.clone(), &path.input_token) {
                        path.optimal_path = kind;
                        let mut w = lookup.lock().unwrap();
                        if let Some(mut existing) = w.get_mut(&pool) {
                            existing.push(path);
                        } else {
                            let mut set = vec![];
                            set.push(path);
                            w.insert(pool.clone(), set);
                        }
                    } else {
                        return
                    }

                
            })
        });
        let mut w = path_lookup1.write().await;
        *w = lookup.lock().unwrap().clone();
    }

    let mut total_paths = 0;
    for (_pool, paths) in path_lookup1.read().await.clone() {
        total_paths += paths.len();
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
    let node_url = "http://89.58.31.215:8545".to_string();
    let gas_lookup = gas_map.clone();

    let signer = PRIVATE_KEY.clone().parse::<LocalWallet>().unwrap();
    let signer_wallet_address = signer.address();
    let provider = ethers_providers::Provider::<Http>::connect(&node_url).await;
    #[cfg(feature = "ipc")]
    let provider = ethers_providers::Provider::<ethers_providers::Ipc>::connect_ipc(&IPC_PATH.clone()).await.unwrap();

    let block = provider
        .get_block(BlockNumber::Latest)
        .await
        .unwrap()
        .unwrap();
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
        let block = block.clone();
        join_handles.push(tokio::runtime::Handle::current().spawn(async move {
            let client = client.clone();
            let mut ix_data = "".to_string();
            let packed_asset = MevPath::encode_packed(I256::from(10000));

            let function = match pool.provider.id() {
                LiquidityProviderId::UniswapV2
                | LiquidityProviderId::SushiSwap
                | LiquidityProviderId::Solidly
                | LiquidityProviderId::CroSwap
                | LiquidityProviderId::ShibaSwap
                | LiquidityProviderId::SaitaSwap
                | LiquidityProviderId::ConvergenceSwap
                | LiquidityProviderId::CurvePlain
                | LiquidityProviderId::FraxSwap
                | LiquidityProviderId::WiseSwap
                | LiquidityProviderId::UnicSwap
                | LiquidityProviderId::ApeSwap
                | LiquidityProviderId::DXSwap
                | LiquidityProviderId::LuaSwap
                | LiquidityProviderId::ElcSwap
                | LiquidityProviderId::CapitalSwap
                | LiquidityProviderId::Pancakeswap
                | LiquidityProviderId::UniswapV3 => {
                    ix_data = pool.provider.pay_self_signature(false)
                        + &pool.address[2..]
                        + if pool.x_to_y { "01" } else { "00" }
                        + &(packed_asset.len() as u8).encode_hex()[64..]
                        + &packed_asset
                    +"00000080";
                    // + if pool.x_to_y {&pool.y_address[2..]} else {&pool.x_address[2..]}
                    // + "0201";
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
                max_fee_per_gas: calculate_next_block_base_fee(block).ok(),
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
                w.insert(pool.address.clone(), gas_used + U256::from(15000));
                debug!("{} uses {:?} {} {:?}", pool.address, gas_used + U256::from(5000),ix_data, tx)
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

    let mut workers = vec![];
    let cores = num_cpus::get();

    let gas_lookup = gas_map.clone();
    used_oneshot.send(uniq_locked).unwrap();
    tokio::time::sleep(Duration::from_secs(30)).await;

    while updated_q.len() != 0 {
        drop(updated_q.recv().await);
    }
    for i in 0..10 {
        let gas_lookup = gas_lookup.clone();
        let path_lookup = path_lookup1.clone();
        let single_routes = single_routes.clone();
        let updated_q = updated_q.clone();
        let locked_pools = locked_pools.clone();
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
                let locked_pools = locked_pools.clone();

                let mut futs = FuturesUnordered::new();

                market_routes
                    .clone()
                    .into_iter()
                    .map(|path| path.pools.clone())
                    .flatten()
                    .unique()
                    .for_each(|pl| {
                        futs.push(async {
                            if pl.x_to_y {
                                let locked = locked_pools.get(&pl).unwrap();
                                drop(pl);
                                locked[0].read().await.clone()
                            } else {
                                let locked = locked_pools.get(&pl).unwrap();
                                drop(pl);
                                locked[1].read().await.clone()
                            }
                        })
                    });
                let needed_reads = futs
                    .collect::<Vec<Pool>>()
                    .await
                    .into_iter()
                    .map(|pool| (pool.address.clone(), pool))
                    .collect::<HashMap<String, Pool>>();
                let r = gas_lookup.read().unwrap().clone();
                let mut updated = market_routes
                    .into_par_iter()
                    .filter_map(|route| {
                        let r = r.clone();
                        let mut pools = vec![];
                        for pool in &route.pools {
                            let mut p = needed_reads.get(&pool.address).unwrap().clone();
                            p.x_to_y = pool.x_to_y;
                            pools.push(p)
                        }
                        if let Some((tx, result)) = route.get_transaction_sync(pools) {
                            let mut gas_cost = U256::zero();
                            for pool in &route.pools {
                                gas_cost += *r.get(&pool.address).unwrap_or(&U256::from(100000));
                            }
                            drop(r);
                            Some(ArbPath {
                                path: route,
                                tx,
                                profit: U256::from(result.profit),
                                gas_cost,
                                block_number: event.block_number,
                                result,
                            })
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<ArbPath>>();
                let mut w = single_routes.lock().unwrap();
                w.send(updated).unwrap();
            }
        }));
    }

    // print some metrics
    workers.push(tokio::spawn(async move {
        loop {
            warn!("Queued updates: {}", updated_q.len());
            tokio::time::sleep(Duration::from_secs(60)).await;
        }

    }));
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
pub fn calculate_next_block_base_fee(block: Block<TxHash>) -> anyhow::Result<U256> {
    // Get the block base fee per gas
    let base_fee = block.base_fee_per_gas.unwrap();

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
