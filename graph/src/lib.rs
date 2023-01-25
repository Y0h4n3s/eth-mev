use std::collections::{HashMap, HashSet};
use async_std::sync::Arc;
use garb_sync_eth::{EventSource, Pool};
use petgraph::algo::all_simple_paths;
use petgraph::prelude::{Graph, NodeIndex};
use petgraph::Undirected;
use tokio::runtime::Runtime;
use tokio::sync::{RwLock, Semaphore};
use once_cell::sync::Lazy;


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
    pub size: u64,
    pub decimals: u64,
    pub route: Vec<Pool>
}

pub struct GraphConfig {
    pub from_file: bool,
    pub save_only: bool
}
pub static CHECKED_COIN: Lazy<String> =  Lazy::new(|| {
    std::env::var("ETH_CHECKED_COIN").unwrap_or("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string())
});

pub static MAX_SIZE: Lazy<u64> = Lazy::new(|| {
    std::env::var("ETH_MAX_SIZE").unwrap_or("100".to_string()).parse().unwrap()
});


pub async fn start(
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    updated_q: kanal::AsyncReceiver<Box<dyn EventSource<Event = Pool>>>,
    routes: Arc<RwLock<kanal::AsyncSender<Order>>>,
    config: GraphConfig,
) -> anyhow::Result<()> {
    
    let mut the_graph: Graph<String, Pool, Undirected> =
          Graph::<String, Pool, Undirected>::new_undirected();
    let pr = pools.read().await;
    for (_, pool) in pr.iter() {
        let index1 = the_graph
              .node_indices()
              .find(|i| the_graph[*i] == pool.x_address.clone());
        let index2 = the_graph
              .node_indices()
              .find(|i| the_graph[*i] == pool.y_address.clone());
        let i1 = if index1.is_none() {
            the_graph.add_node(pool.x_address.clone())
        } else {
            index1.unwrap()
        };
        let i2 = if index2.is_none() {
            the_graph.add_node(pool.y_address.clone())
        } else {
            index2.unwrap()
        };
        if the_graph
              .edges_connecting(i1, i2)
              .find(|e| {
                  e.weight().y_address == pool.y_address
                        && e.weight().address == pool.address
                        && e.weight().provider == pool.provider
                        && e.weight().x_address == pool.x_address
              })
              .is_none()
        {
            
            the_graph.add_edge(i1, i2, pool.clone());
        }
    }
    println!(
        "graph service> Preparing routes {} ",
        the_graph.node_count(),
    );
    
    let mut checked_coin_indices: Vec<NodeIndex> = vec![];
    
    if let Some(index) = the_graph
          .node_indices()
          .find(|i| the_graph[*i] == CHECKED_COIN.clone())
    {
        checked_coin_indices.push(index);
    } else {
        println!(
            "graph service> Skipping {} because there are no pools with that coin",
            CHECKED_COIN.clone()
        );
    }
    
    
    let path_lookup = Arc::new(RwLock::new(
        HashMap::<Pool, HashSet<(String, Vec<Pool>)>>::new(),
    ));
    
    if config.from_file {
	    println!("graph_service> Loading routes from file");
	    let config = bincode::config::standard();
		let contents = std::fs::read_to_string("path_lookup_1_uniswapv2_uniswapv1.json")?;
		let mut path_lookup = path_lookup.write().await;
		let encoded: Vec<u8> = serde_json::from_str(&contents)?;
	
	    let (decoded, len): (HashMap<Pool, HashSet<(String, Vec<Pool>)>>, usize) = bincode::decode_from_slice(&encoded[..], config).unwrap();
	    *path_lookup = decoded;
    } else {
	    let mut two_step_routes = HashSet::<(String, Vec<Pool>)>::new();
	
	    // two step routes first
	    for node in the_graph.node_indices() {
		    for checked_coin in &*checked_coin_indices {
			    let in_address = the_graph.node_weight(*checked_coin).unwrap().to_string();
			
			    two_step_routes = two_step_routes
				      .union(
					      &the_graph
							    .neighbors(node.clone())
							    .map(|p| {
								    let neighbor = the_graph.node_weight(p).unwrap();
								    if *neighbor == in_address {
									    let edges_connecting = the_graph.edges_connecting(node.clone(), p);
									    if edges_connecting.clone().count() >= 2 {
										    let mut pools: Vec<Pool> = vec![];
										    // println!("graph service> Found route for {} to {} via {}", node_addr, neighbor, edges_connecting.clone().count());
										
										    for edge in edges_connecting {
											    pools.push(Pool::from(edge.weight()));
										    }
										    let combined_routes = combinations(pools.as_mut_slice(), 2);
										    return combined_routes
											      .iter()
											      .filter(|path| {
												      let first_pool = path.first().unwrap();
												      let last_pool = path.last().unwrap();
												      if (first_pool.x_address == in_address
														    && last_pool.y_address == in_address)
														    || (first_pool.y_address == in_address
														    && last_pool.x_address == in_address)
												      {
													      return true;
												      } else {
													      return false;
												      }
											      })
											      .map(|path| (in_address.clone(), path.clone()))
											      .collect::<Vec<(String, Vec<Pool>)>>();
									    } else {
										    return vec![];
									    }
								    } else {
									    vec![]
								    }
							    })
							    .filter(|p| p.len() > 0)
							    .flatten()
							    .collect::<HashSet<(String, Vec<Pool>)>>(),
				      )
				      .map(|i| i.clone())
				      .collect();
		    }
	    }
	    let cores = num_cpus::get();
	    let permits = Arc::new(Semaphore::new(cores));
	    let edges_count = the_graph.edge_count();
	    for (i, edge) in the_graph.edge_indices().enumerate() {
		    let permit = permits.clone().acquire_owned().await.unwrap();
		    println!("graph service> Preparing routes {} / {}", i, edges_count);
		    let the_graph = the_graph.clone();
		    let checked_coin_indices = checked_coin_indices.clone();
		    let two_step_routes = two_step_routes.clone();
		    let path_lookup = path_lookup.clone();
		    tokio::spawn(async move {
			    let edge = the_graph.edge_weight(edge).unwrap();
			
			    let index1 = the_graph
				      .node_indices()
				      .find(|i| the_graph[*i] == edge.x_address)
				      .unwrap();
			    let index2 = the_graph
				      .node_indices()
				      .find(|i| the_graph[*i] == edge.y_address)
				      .unwrap();
			    // println!("graph service> Finding routes for {}", edge);
			
			    let updated_nodes = vec![index1, index2];
			    let mut safe_paths: HashSet<(String, Vec<Pool>)> = HashSet::new();
			
			    for node in updated_nodes {
				    for checked_coin in &*checked_coin_indices {
					    let in_address = the_graph.node_weight(*checked_coin).unwrap().to_string();
					
					    // TODO: make max_intermediate_nodes and min_intermediate_nodes configurable
					    // find all the paths that lead to the current checked coin from the updated coin
					    // max_intermediate_nodes limits the number of swaps we make, it can be any number but the bigger
					    // the number the more time it will take to find the paths
					    let to_checked_paths = all_simple_paths::<Vec<NodeIndex>, _>(
						    &the_graph,
						    node,
						    *checked_coin,
						    0,
						    Some(1),
					    )
						      .collect::<Vec<_>>();
					
					    for (i, ni) in to_checked_paths.iter().enumerate() {
						    for (j, nj) in to_checked_paths.iter().enumerate() {
							    // skip routing back and forth
							    if i == j {
								    continue;
							    }
							
							    // eg. assuming the checked coin is wormhole usdc and the other coin is apt
							    //     ni: [apt -> via aux -> usdd -> via liquidswap -> usdc]
							    //     for each nj: [[apt -> via animeswap -> usdc],[apt -> via aux -> mojo -> via aux -> usdc],...]
							    //
							    // p1 -> reverse -> pop = [usdc -> via liquidswap -> usdd]
							    // new_path = [usdc -> via liquidswap -> usdd -> via aux -> apt -> via animeswap -> usdc]
							    let mut p1 = ni.clone();
							    p1.reverse();
							    p1.pop();
							
							    let new_path: Vec<&NodeIndex> = p1.iter().chain(nj).collect();
							    // println!("{}", new_path.len());
							    if new_path.len() < 4 {
								    continue;
							    }
							    // collect all the pools between the coins
							    // combine the edges
							    let mut edge_paths: Vec<Vec<Pool>> = vec![];
							    for (i, node) in new_path.iter().enumerate() {
								    if i == 0 {
									    continue;
								    }
								    let edges = the_graph.edges_connecting(**node, *new_path[i - 1]);
								
								    if edge_paths.len() <= 0 {
									    for edge in edges.clone() {
										    let mut pool = edge.weight().clone();
										    pool.x_to_y = pool.x_address == in_address;
										    edge_paths.push(vec![pool]);
									    }
									    continue;
								    }
								    let mut new_edge_paths = vec![];
								
								    for old_path in edge_paths {
									    let last_pool = old_path.last().unwrap();
									    let in_a = if last_pool.x_to_y {
										    last_pool.y_address.clone()
									    } else {
										    last_pool.x_address.clone()
									    };
									    for edge in edges.clone() {
										    let mut pool = edge.weight().clone();
										    pool.x_to_y = pool.x_address == in_a;
										    new_edge_paths.push(
											    old_path
												      .clone()
												      .into_iter()
												      .chain(vec![pool])
												      .collect(),
										    );
									    }
								    }
								    edge_paths = new_edge_paths;
							    }
							
							    'add: for mut edge_path in edge_paths {
								    // if consecutive pools are the same, skip
								    for i in 1..edge_path.len() {
									    if edge_path[i - 1].x_address == edge_path[i].x_address
										      && edge_path[i - 1].y_address == edge_path[i].y_address
										      && edge_path[i - 1].provider == edge_path[i].provider
										      && edge_path[i - 1].address == edge_path[i].address
									    {
										    continue 'add;
									    }
								    }
								    safe_paths.insert((in_address.clone(), edge_path.clone()));
								
								    // add the reverse of each path just to be thorough
								    edge_path.reverse();
								    let mut new_path = vec![];
								    let mut in_a = in_address.clone();
								    for pool in edge_path {
									    let mut new_pool = pool.clone();
									    new_pool.x_to_y = new_pool.x_address == in_a;
									    in_a = if new_pool.x_to_y {
										    new_pool.y_address.clone()
									    } else {
										    new_pool.x_address.clone()
									    };
									    new_path.push(new_pool);
								    }
								    // println!("pools: {}", new_path.len());
								    safe_paths.insert((in_address.clone(), new_path));
							    }
						    }
					    }
				    }
			    }
			    // use only paths that route through the current edge
			    safe_paths = safe_paths
				      .into_iter()
				      .filter(|(_in, path)| {
					      path.iter().any(|p| {
						      p.address == edge.address
								    && p.x_address == edge.x_address
								    && p.y_address == edge.y_address
								    && p.provider == edge.provider
					      })
				      })
				      //       .map(|(in_addr, path)| {
				      //     let mut first_pool = path.first().unwrap().clone();
				      //     let mut second_pool = path.get(1).unwrap().clone();
				      //     let mut last_pool = path.last().unwrap().clone();
				      //
				      //     first_pool.x_to_y = first_pool.x_address == CHECKED_COIN.clone();
				      //     last_pool.x_to_y = last_pool.x_address == CHECKED_COIN.clone();
				      //     return (in_addr, vec![first_pool.clone(), second_pool.clone(), last_pool.clone()]);
				      // })
				      .collect();
			    let two_step = two_step_routes
				      .clone()
				      .into_iter()
				      .filter(|(_in_addr, path)| {
					      path.iter().any(|p| {
						      p.address == edge.address
								    && p.x_address == edge.x_address
								    && p.y_address == edge.y_address
								    && p.provider == edge.provider
					      })
				      })
				      .map(|(in_addr, path)| {
					      let mut first_pool = path.first().unwrap().clone();
					      let mut second_pool = path.last().unwrap().clone();
					      if first_pool.x_address != in_addr {
						      first_pool.x_to_y = false;
					      }
					      if second_pool.x_address != in_addr {
						      second_pool.x_to_y = false;
					      }
					      return (in_addr, vec![first_pool.clone(), second_pool.clone()]);
				      })
				      .collect::<HashSet<(String, Vec<Pool>)>>();
			    safe_paths.extend(two_step);
			    let mut w = path_lookup.write().await;
			    w.insert(Pool::from(edge), safe_paths);
			    std::mem::drop(permit);
		    });
	    }
	    if config.save_only {
		    let config = bincode::config::standard();
		    let file = std::fs::File::create(std::path::PathBuf::from("path_lookup.json"))?;
		    let encoded: Vec<u8> = bincode::encode_to_vec(&path_lookup.read().await.clone(), config)?;
		    serde_json::to_writer(file, &encoded)?;
		    return Ok(())
	    }
	   
    }
	let mut total_paths = 0;
	for (_pool, paths) in path_lookup.read().await.clone() {
		total_paths += paths.len();
		// for (forf   , path) in paths {
		//     println!("`````````````````````` Tried Route ``````````````````````");
		//     for (i, pool) in path.iter().enumerate() {
		//         println!("{}. {}", i + 1, pool);
		//     }
		//     println!("\n\n");
		// }
	}
    println!("graph service> Found {} routes", total_paths);
    
    
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
                let updated_market = updated_market_event.get_event();
                let path_lookup = path_lookup.clone();
                let routes = routes.clone();
                tokio::task::spawn_local(async move {
                    let read = path_lookup.read().await;
                    let updated = read.iter().find(|(key, _value)|updated_market.address == key.address && updated_market.x_address == key.x_address && updated_market.y_address == key.y_address && updated_market.provider == key.provider);
                    if updated.is_some() {
                        let (pool, market_routes) =  updated.unwrap();
	                    println!("graph service> Found {} routes for updated market", market_routes.len());
                        let pool = pool.clone();
                        let market_routes = market_routes.clone();
                        std::mem::drop(read);
                        if market_routes.len() <= 0  {
                            return;
                        }
                        
                        
                        let mut new_market_routes = HashSet::new();
                        for (_pool_addr, mut paths) in market_routes.into_iter() {
                            if !(paths.iter().find(|p| p.address == updated_market.address && p.x_address == updated_market.x_address && p.y_address == updated_market.y_address && p.provider == updated_market.provider ).is_some()) {
                                new_market_routes.insert((_pool_addr, paths));
                                continue
                            }
                            let pool_index = paths.iter().position(|p| p.address == updated_market.address && p.x_address == updated_market.x_address && p.y_address == updated_market.y_address && p.provider == updated_market.provider ).unwrap();
                            paths[pool_index] = updated_market.clone();
                            new_market_routes.insert((_pool_addr, paths.clone()));
                            
                            let in_addr = if paths.first().unwrap().x_to_y {
                                paths.first().unwrap().x_address.clone()
                            } else {
                                paths.first().unwrap().y_address.clone()
                            };
                            let decimals = decimals(in_addr);
                            
                            let mut best_route_index = 0;
                            let mut best_route = 0.0;
	                        
	                        // Todo: use binary search or quadratic searchhere
                            for i in 1..MAX_SIZE.clone()+1 {
                                let i_atomic = (i as u128) * 10_u128.pow(decimals as u32);
                                let mut in_ = i_atomic;
                                for route in &paths {
                                    let calculator = route.provider.build_calculator();
                                    
                                    in_ = calculator.calculate_out(in_, route);
                                }
                                if in_ < i_atomic {
                                    continue;
                                }
	                            
                                
                                let percent = in_ as f64 - i_atomic as f64;
                                
                                
                                if percent > best_route {
                                    best_route = percent;
                                    best_route_index = i;
	                                // println!("graph service> Found route with in: {} out: {} at index {} with {}%", i_atomic, in_, best_route_index, percent);
	
                                }
                            }
                            
                            if best_route > 0.0 {
                                let order = Order {
                                    size: best_route_index as u64,
                                    decimals,
                                    route: paths.clone(),
                                };
                                
                                let r = routes.write().await;
                                r.try_send(order).unwrap();
                            }
                        }
                        let mut w = path_lookup.write().await;
                        
                        w.insert(pool.clone(), new_market_routes);
                    } else {
                        eprintln!("graph service> No routes found for {}", updated_market);
                    }
                });
            }
        });
        
    }).join().unwrap();
    
    
    Ok(())
}
fn decimals(coin: String) -> u64 {
    match coin.as_str() {
        "0x1::aptos_coin::AptosCoin" => {
            8
        }
        _ => {6}
    }
}
