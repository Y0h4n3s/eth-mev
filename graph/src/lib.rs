pub mod mev_path;
use async_std::sync::Arc;
use bb8_bolt::bolt_client::Params;
use bb8_bolt::{
    bb8::Pool as BPool,
    bolt_client::Metadata,
    bolt_proto::{version::*, Value},
    Manager,
};
use bolt_proto::value::{Node, Relationship};
use bolt_proto::Message;
use garb_sync_eth::{EventSource, LiquidityProviderId, LiquidityProviders, Pool};
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

pub fn all_simple_paths_non_circular<TargetColl>(
    g: &Graph<String, Pool, Undirected>,
    from: NodeIndex,
    to: NodeIndex,
    min_intermediate_nodes: usize,
    max_intermediate_nodes: usize,
) -> Vec<Vec<NodeIndex>> {
    let mut dfs = Bfs::new(g, from);
    let mut found_paths = vec![];
    while let Some(nx) = dfs.next(g) {
        if nx == to {
            found_paths.push(Vec::from(dfs.stack.clone()));
        } else {
            // println!("len {}", dfs.stack.len());
            if dfs.stack.len() > 10 {
                dfs.stack.pop_front();
            }
        }
    }
    found_paths
}

pub async fn start(
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    updated_q: kanal::AsyncReceiver<Box<dyn EventSource<Event = Pool>>>,
    routes: Arc<RwLock<kanal::AsyncSender<Order>>>,
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

	    // relationship between tokens as liquidity pool
        let res = conn
		          .run(
			          "MATCH (a:Token), (b:Token) WHERE a.address = $x_address  AND b.address = $y_address  MERGE (a)-[r:LP { pool: $pool_address, bn: $provider }]->(b) RETURN type(r)",
			          Some(Params::from_iter(vec![
				          ("x_address", pool.x_address.clone()),
				          ("y_address", pool.y_address.clone()),
				          ("pool_address", pool.address.clone()),
				          ("provider", serde_json::to_string(&pool.provider).unwrap())])),
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
            HashMap::<Pool, HashSet<MevPath>>::new(),
    ));
    if config.from_file {
        println!("graph_service> Loading routes from file");
        let config = bincode::config::standard();
        let contents = std::fs::read_to_string("path_lookup_1_uniswapv2_uniswapv1.json")?;
        let mut path_lookup = path_lookup.write().await;
        let encoded: Vec<u8> = serde_json::from_str(&contents)?;

        let (decoded, len): (HashMap<Pool, HashSet<(String, Vec<Pool>)>>, usize) =
            bincode::decode_from_slice(&encoded[..], config).unwrap();
        *path_lookup = decoded;
    } else {
        let max_intermidiate_nodes = 5;
        let cores = num_cpus::get();
        let permits = Arc::new(Semaphore::new(2));
        let mut handles = vec![];
        for i in 2..max_intermidiate_nodes {
            let permit = permits.clone().acquire_owned().await?;
            println!("graph service> Preparing {} step routes ", i);
            let path_lookup = path_lookup.clone();
            let path_lookup1 = path_lookup1.clone();
            let pools = pools.clone();
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
			    let mut conn = pool.get().await?;
			
			    let res = conn.run(format!("match cyclePath=(m1:Token{{address:'{}'}})-[*{}..{}]-(m2:Token{{address:'{}'}}) RETURN relationships(cyclePath) as cycle", CHECKED_COIN.clone(), i, i,CHECKED_COIN.clone()), None, None).await?;
			    let pull_meta = Metadata::from_iter(vec![("n", 1000)]);
			    let (mut records, mut response) = conn.pull(Some(pull_meta.clone())).await?;
			    loop {
				    // populate path_lookup with this batch
				    for record in &records {
					    let ps = pools.read().await;
					    let mut pools = record.fields().iter().filter_map(|val|  {
						
						    match val {
							    Value::List(rels) => {
								    let mut r = vec![];
								    for rel in rels {
									    match rel {
										    Value::Relationship(rel) => {
											    let address = match rel.properties().get("pool") {
												    Some(Value::String(s)) => {s.clone()}
												    _ => "0x0".to_string()
											    };
											    let provider = match rel.properties().get("bn") {
												    Some(Value::String(provider)) => {LiquidityProviders::from(provider)}
												    _ => LiquidityProviders::UniswapV2(Default::default())
											    };
											    match ps.iter().find(|(_, p)| p.address == address && p.provider == provider ) {
												    Some((s, pool)) => r.push(pool.clone()),
												    _ => ()
											    }
										    }
										    _ => ()
									    }
									    
								    }
								    if r.len() != rels.len() {
									    None
								    } else {
									    let path = MevPath::new(r.clone());
									    Some(path)
								    }
								    
							    }
							    _ => None
								 
							 }
                        }).collect::<Vec<MevPath>>();
					    for p in pools {
						    let mut in_ = CHECKED_COIN.clone();

						    for pool in p.path.iter() {
							    let mut w = path_lookup1.write().await;
							    if let Some(mut existing) = w.get_mut(&pool) {
								    existing.insert( p.clone());
							    } else {
								    let mut set = HashSet::new();
								    set.insert( p.clone());
								    w.insert(pool.clone(), set);
							    }
						    }
					    }
					    
				    }
				    
				    // query next batch from stream
				    match &response {
					    
					    Message::Success(success) => {
						    if let Some(has_more) = success.metadata().get("has_more") {
							    (records,  response) = conn.pull(Some(pull_meta.clone())).await?;
						    } else {
							    break
						    }
					    }
					    _ => break
				    }
			    }
			    
			    println!("Done {} step", i);
			    drop(permit);
			    Ok::<(), anyhow::Error>(())
		    }));

            loop {
                if permits.available_permits() <= 0 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                } else {
                    break;
                }
            }
        }
        for handle in handles {
            handle.await?;
        }

        if config.save_only {
            let config = bincode::config::standard();
            let file = std::fs::File::create(std::path::PathBuf::from("path_lookup.json"))?;
            let encoded: Vec<u8> =
                bincode::encode_to_vec(&path_lookup.read().await.clone(), config)?;
            serde_json::to_writer(file, &encoded)?;
            return Ok(());
        }
    }
    let mut total_paths = 0;
    for (_pool, paths) in path_lookup1.read().await.clone() {
        for path in paths {
        total_paths += path.arrangements.len();

        }
        // for (forf, path) in paths {
        //     println!("`````````````````````` Tried Route ``````````````````````");
        //     for (i, pool) in path.iter().enumerate() {
        //         println!("{}. {}", i + 1, pool);
        //     }
        //     println!("\n\n");
        // }
    }
    println!("graph service> Found {} routes", total_paths);
	return Ok(());
    // println!("graph service> Registering Gas consumption for transactions");
    // for (_pool, paths) in path_lookup.read().await.clone() {
    //     for (in_addr, path) in paths {
    //         let order = Order {
    //             size: MAX_SIZE.clone(),
    //             decimals: decimals(in_addr),
    //             route: path.clone()
    //         };
    //         let r = routes.write().await;
    //         r.try_send(order).unwrap();
    //     }
    //     tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // }
    println!("graph service> Starting Listener thread");
    std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        let task_set = tokio::task::LocalSet::new();
        task_set.block_on(&rt, async {
            while let Ok(updated_market_event) = updated_q.recv().await {
                let mut updated_market = updated_market_event.get_event();
                let path_lookup = path_lookup.clone();
                let routes = routes.clone();
                tokio::task::spawn_local(async move {
                    let read = path_lookup.read().await;
                    let updated = read.iter().find(|(key, _value)| {
                        updated_market.address == key.address
                            && updated_market.x_address == key.x_address
                            && updated_market.y_address == key.y_address
                            && updated_market.provider == key.provider
                    });
                    if updated.is_some() {
                        let (pool, market_routes) = updated.unwrap();
                        // println!(
                        //     "graph service> Found {} routes for updated market {}",
                        //     market_routes.len(),
                        //     updated_market
                        // );
                        let pool = pool.clone();
                        let market_routes = market_routes.clone();
                        std::mem::drop(read);
                        if market_routes.len() <= 0 {
                            return;
                        }

                        let mut new_market_routes = HashSet::new();
                        for (_pool_addr, mut paths) in market_routes.into_iter() {
	                        if !(paths
		                          .iter()
		                          .find(|p| {
			                          p.address == updated_market.address
					                        && p.x_address == updated_market.x_address
					                        && p.y_address == updated_market.y_address
					                        && p.provider == updated_market.provider
		                          })
		                          .is_some())
	                        {
		                        new_market_routes.insert((_pool_addr, paths));
		                        continue;
	                        }
	                        let pool_index = paths
		                          .iter()
		                          .position(|p| {
			                          p.address == updated_market.address
					                        && p.x_address == updated_market.x_address
					                        && p.y_address == updated_market.y_address
					                        && p.provider == updated_market.provider
		                          })
		                          .unwrap();
	                        updated_market.x_to_y = paths[pool_index].x_to_y;
	                        paths[pool_index] = updated_market.clone();
	                        new_market_routes.insert((_pool_addr, paths.clone()));
	
	                        let in_addr = if paths.first().unwrap().x_to_y {
		                        paths.first().unwrap().x_address.clone()
	                        } else {
		                        paths.first().unwrap().y_address.clone()
	                        };
	                        let decimals = decimals(in_addr);

                            let mut best_route_size = 0.0;
                            let mut best_route_profit = 0;
                            let mut mid = MAX_SIZE.clone() / 2.0;
                            let mut left = 0.0;
                            let mut right = MAX_SIZE.clone();
                            // binary search for 10 steps
                            // println!("`````````````````````` Tried Route ``````````````````````");
                            // for (i,pool) in paths.iter().enumerate() {
                            //     println!("{}. {}", i + 1, pool);
                            // }
                            // println!("\n\n");
                            for i in 0..10 {
                                let i_atomic = (mid) * 10_u128.pow(decimals as u32) as f64;
                                let mut in_ = i_atomic as u128;
                                for route in &paths {
                                    let calculator = route.provider.build_calculator();
                                    in_ = calculator.calculate_out(in_, route).unwrap();
                                    // println!("{} {} {}", in_, route.x_amount, route.y_amount);
                                }

                                let profit = (in_ as i128 - i_atomic as i128);

                                if i == 0 {
                                    best_route_profit == profit;
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
                                // println!("Step {}: {} new mid {} ({} - {}) {} {}", i, profit, mid ,left, right, in_, i_atomic);
                            }
                            if best_route_profit > 0 {
                                let order = Order {
                                    size: best_route_size as u128,
                                    decimals,
                                    route: paths.clone(),
                                    profit: best_route_profit as f64,
                                };

                                let r = routes.write().await;
                                r.try_send(order).unwrap();
                            }
                        }
                        let mut w = path_lookup.write().await;

                        w.insert(pool.clone(), new_market_routes);
                    } else {
                        // eprintln!("graph service> No routes found for {}", updated_market);
                    }
                });
            }
        });
    })
    .join()
    .unwrap();

    Ok(())
}
fn decimals(coin: String) -> u64 {
    match coin.as_str() {
        "0x1::aptos_coin::AptosCoin" => 18,
        _ => 18,
    }
}
