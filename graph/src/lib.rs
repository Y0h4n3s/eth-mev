#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_must_use)]
#![allow(non_snake_case)]
#![allow(unreachable_patterns)]
#![allow(unused)]


pub mod mev_path;
use ethers::types::Transaction;
use async_std::sync::Arc;
use bb8_bolt::bolt_client::Params;
use bb8_bolt::{
    bb8::Pool as BPool,
    bolt_client::Metadata,
    bolt_proto::{version::*, Value},
    Manager,
};
use tracing::{debug, info};
use rayon::prelude::*;
use bolt_proto::value::{Node, Relationship};
use bolt_proto::Message;
use garb_sync_eth::{EventSource, LiquidityProviderId, LiquidityProviders, PendingPoolUpdateEvent, Pool, PoolUpdateEvent};
use neo4rs::{query, Graph as NGraph};
use once_cell::sync::Lazy;
use petgraph::algo::all_simple_paths;
use petgraph::prelude::{Graph, NodeIndex};
use petgraph::visit::{Bfs, Dfs};
use petgraph::{
    visit::{IntoNeighborsDirected, NodeCount},
    Outgoing, Undirected,
};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::iter::from_fn;
use ethers::abi::AbiEncode;
use ethers::types::{Eip1559TransactionRequest, U256};
use tokio::runtime::Runtime;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::Duration;
use crate::mev_path::MevPath;

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
    updated_q: kanal::AsyncReceiver<Box<dyn EventSource<Event=PoolUpdateEvent>>>,
    pending_updated_q: kanal::AsyncReceiver<Box<dyn EventSource<Event=PendingPoolUpdateEvent>>>,
    routes: Arc<RwLock<kanal::AsyncSender<(Transaction, Eip1559TransactionRequest)>>>,
    config: GraphConfig,
) -> anyhow::Result<()> {
    Graph::<String, Pool, Undirected>::new_undirected();
    let pr = pools.read().await;

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
    let pool = Arc::new(BPool::builder().build(manager).await?);

    // Fetch and use a connection from the pool
    let mut conn = pool.get().await?;
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
    for (_, pool) in pr.iter() {
        let res = conn
            .run(
                "MERGE (t:Token {address: $address}) RETURN t",
                Some(Params::from_iter(vec![
                    ("address", pool.x_address.clone()),
                ])),
                None,
            )
            .await?;
        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;

        let res = conn
            .run(
                "MERGE (t:Token {address: $address}) RETURN t",
                Some(Params::from_iter(vec![
                    ("address", pool.y_address.clone()),
                ])),
                None,
            )
            .await?;

        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;

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
            .await?;

        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;

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
            .await?;

        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;

        // relationship between tokens as liquidity pool
        // let res = conn
        //           .run(
        // 	          "MATCH (a:Token), (b:Token) WHERE a.address = $x_address  AND b.address = $y_address  MERGE (a)-[r:LP { pool: $pool_address, bn: $provider }]->(b) RETURN type(r)",
        // 	          Some(Params::from_iter(vec![
        // 		          ("x_address", pool.x_address.clone()),
        // 		          ("y_address", pool.y_address.clone()),
        // 		          ("pool_address", pool.address.clone()),
        // 		          ("provider", serde_json::to_string(&pool.provider).unwrap())])),
        // 	          None).await?;
        //
        // let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        // let (records, response) = conn.pull(Some(pull_meta)).await?;

        let res = conn
            .run(
                "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $y_address  MERGE (a)-[r:ASSET]->(b) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", true.to_string()),
                    ("y_address", pool.y_address.clone()),
                ])),
                None).await?;

        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;
        let res = conn
            .run(
                "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $x_address  MERGE (b)-[r:DEBT]->(a) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", true.to_string()),
                    ("x_address", pool.x_address.clone()),
                ])),
                None).await?;

        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;


        let res = conn
            .run(
                "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $x_address  MERGE (a)-[r:ASSET]->(b) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", false.to_string()),
                    ("x_address", pool.x_address.clone()),
                ])),
                None).await?;

        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;

        let res = conn
            .run(
                "MATCH (a:Pool), (b:Token) WHERE a.address = $pool_address AND a.x_to_y = $pool_x_to_y  AND b.address = $y_address  MERGE (b)-[r:DEBT]->(a) RETURN type(r)",
                Some(Params::from_iter(vec![
                    ("pool_address", pool.address.clone()),
                    ("pool_x_to_y", false.to_string()),
                    ("y_address", pool.y_address.clone()),
                ])),
                None).await?;

        let pull_meta = Metadata::from_iter(vec![("n", -1)]);
        let (records, response) = conn.pull(Some(pull_meta)).await?;
    }
    drop(pr);

    // txn.run_queries(queries).await.unwrap();
    // txn.commit().await.unwrap();

    let mut checked_coin_indices: Vec<NodeIndex> = vec![];

    let path_lookup = Arc::new(RwLock::new(
        HashMap::<Pool, HashSet<(String, Vec<Pool>)>>::new(),
    ));
    let path_lookup1 = Arc::new(RwLock::new(
        HashMap::<Pool, Vec<MevPath>>::new(),
    ));
        let max_intermidiate_nodes = 4;

        for i in 2..max_intermidiate_nodes {
            info!("Preparing {} step routes ", i);
            let path_lookup = path_lookup.clone();
            let pool = pool.clone();
            let mut conn = pool.get().await?;
            let cores = num_cpus::get();
            let permits = Arc::new(Semaphore::new(25));
            // OLD MATCHER: match cyclePath=(m1:Token{{address:'{}'}})-[*{}..{}]-(m2:Token{{address:'{}'}}) RETURN relationships(cyclePath) as cycle, nodes(cyclePath)
            let mut steps = "".to_string();
            let mut where_clause = "1 = 1".to_string();
            for j in 0..i {
                steps = steps + &format!("-[:DEBT]-(p{}:Pool)-[:ASSET]", j);
                if j != i - 1 {
                    steps = steps + &format!("-(t{}:Token)", j);
                    for k in 0..i-1 {
                        if k == j {
                            continue
                        }
                        where_clause += &format!(" AND t{}.address <> t{}.address", j, k)
                    }
                    for k in 0..i {
                        if k == j {
                            continue
                        }
                        where_clause += &format!(" AND p{}.address <> p{}.address", j, k)
                    }
                    where_clause += &format!(" AND t.address <> t{}.address", j)
                }


            }
            let query= format!("MATCH path=(t:Token{{address: '{}'}}){}->(t) WHERE {} return nodes(path);", CHECKED_COIN.clone(), steps, where_clause);
            info!("{}", query);
            let res = conn.run(query, None, None).await?;
            let pull_meta = Metadata::from_iter(vec![("n", 10000)]);
            let (mut records, mut response) = conn.pull(Some(pull_meta.clone())).await?;
            loop {
                let mut handles = vec![];

                // populate path_lookup with this batch
                for record in records {
                    let permit = permits.clone().acquire_owned().await?;
                    let path_lookup1 = path_lookup1.clone();
                    let pools = pools.clone();
                    handles.push(tokio::spawn(async move {
                        let ps = pools.read().await;
                        let mut pools = record.fields().iter().filter_map(|val| {
                            match val {
                                Value::List(rels) => {
                                    let mut r = vec![];
                                    for rel in rels {
                                        match rel {
                                            Value::Node(rel) => {
                                                if let Some(is_pool) = rel.labels().iter().find(|p| *p == &"Pool".to_string()) {
                                                    let address = match rel.properties().get("address") {
                                                        Some(Value::String(s)) => { s.clone() }
                                                        _ => "0x0".to_string()
                                                    };
                                                    let provider = match rel.properties().get("provider") {
                                                        Some(Value::String(provider)) => { LiquidityProviders::from(provider) }
                                                        _ => LiquidityProviders::UniswapV2(Default::default())
                                                    };
                                                    let x_to_y = match rel.properties().get("x_to_y") {
                                                        Some(Value::String(x_to_y)) => {
                                                            if x_to_y == &"true".to_string() {
                                                                true
                                                            } else {
                                                                false
                                                            }
                                                        }
                                                        _ => false
                                                    };
                                                    match ps.iter().find(|(_, p)| p.address == address && p.provider == provider) {
                                                        Some((s, pool)) => {
                                                            let mut new = pool.clone();
                                                            new.x_to_y = x_to_y;
                                                            r.push(new)
                                                        }
                                                        _ => ()
                                                    }
                                                }
                                            }

                                            _ => ()
                                        }
                                    }
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
                                        let mut path = MevPath::new(&r, &CHECKED_COIN.clone());
                                        for pool in r {
                                            path.update(pool);
                                        }
                                        if path.is_valid() {

                                            Some(path)
                                        } else {
                                            None
                                        }
                                    }
                                }
                                _ => None
                            }
                        }).collect::<Vec<MevPath>>();
                        for p in pools {
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
                                        (records, response) = conn.pull(Some(pull_meta.clone())).await?;

                                    } else {
                                        break
                                    }
                                }
                                _ => break
                            }
                        } else {
                            break;
                        }
                    }
                    _ => break
                }
            }


            let mut total_paths = 0;
            for (_pool, paths) in path_lookup1.read().await.clone() {
                for path in paths {
                    total_paths += path.paths.len();
                }
                // for (forf, path) in paths {
                //     info!("`````````````````````` Tried Route ``````````````````````");
                //     for (i, pool) in path.iter().enumerate() {
                //         info!("{}. {}", i + 1, pool);
                //     }
                //     info!("\n\n");
                // }
            }
            info!("Done {} step {}", i, total_paths);
        }



    let mut total_paths = 0;
    for (_pool, paths) in path_lookup1.read().await.clone() {
        for path in paths {
            total_paths += path.paths.len();
        }

        // for (forf, path) in paths {
        //     info!("`````````````````````` Tried Route ``````````````````````");
        //     for (i, pool) in path.iter().enumerate() {
        //         info!("{}. {}", i + 1, pool);
        //     }
        //     info!("\n\n");
        // }
    }
    info!("Found {} routes", total_paths);
//	return Ok(());
//     info!("Registering Gas consumption for transactions");
//     for (_pool, paths) in path_lookup1.read().await.clone() {
//         for mut route in paths.iter() {
//             let mut r = route.clone();
//             for pool in route.pools.iter() {
//                r.update(pool.clone());
//             }
//         }
//         tokio::time::sleep(std::time::Duration::from_millis(500)).await;
//     }
    info!("Starting Listener thread");
    info!("Clearing {} cached events", updated_q.len() + pending_updated_q.len());

    while !pending_updated_q.is_empty() {
        drop(pending_updated_q.recv().await);
    }

    let mut workers = vec![];
    let cores = num_cpus::get();

    for i in 0..cores {
        let path_lookup = path_lookup1.clone();
        let routes = routes.read().await.clone();
        let updated_q = updated_q.clone();
        workers.push(tokio::spawn(async move {
            while let Ok(updated_market_event) = updated_q.recv().await {
                let event = updated_market_event.get_event();
                let mut updated_market = event.pool;
                let routes = routes.clone();
                if let Some((pool, market_routes)) = path_lookup.write().await.iter_mut().find(|(key, _value)| {
                    updated_market.address == key.address
                }) {
                    for mut route in market_routes.iter_mut() {
                        route.update(updated_market.clone());
                    }

                    debug!(
                        "Updating {} routes for updated market {}",
                        market_routes.len(),
                        updated_market,
                    );
                    continue
                }
                else {
                    debug!("No routes found for {}", updated_market);
                    continue;
                };

            }
            }));
    }

    for i in 0..cores {
        let path_lookup = path_lookup1.clone();
        let routes = routes.read().await.clone();
        let pending_updated_q = pending_updated_q.clone();
        workers.push(tokio::spawn(async move {
            while let Ok(updated_market_event) = pending_updated_q.recv().await {
                let event = updated_market_event.get_event();
                let mut updated_market = event.pool;
                let routes = routes.clone();
                let market_routes = if let Some((pool, market_routes)) = path_lookup.read().await.iter().find(|(key, _value)| {
                    updated_market.address == key.address
                }) {
                    debug!(
                        "Found {} routes for pending transaction {}",
                        market_routes.len(),
                        event.pending_tx.hash.encode_hex()
                    );
                    if market_routes.len() <= 0 {
                        continue
                    }
                    market_routes.clone()
                    }
                else {
                    debug!("No routes found for pending update {}", updated_market);
                    continue;
                };
                let mut updated = market_routes.into_par_iter().map(|mut route| {
                    let transactions = route.get_transaction_for_pending_update(updated_market.clone());
                  transactions
                }).flatten().collect::<Vec<Eip1559TransactionRequest>>();
                if updated.len() == 0 {
                    continue
                }
                for tx in updated {
                    routes.send((event.pending_tx, tx)).await;

                }
            }
        }));
    }
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
