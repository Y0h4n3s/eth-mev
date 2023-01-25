mod abi;

use once_cell::sync::Lazy;
use abi::Aggregator;
use tokio::runtime::Runtime;
use async_std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::{HashMap};
use ethers::prelude::{Address, H160};
use url::Url;
use garb_sync_eth::{EventSource, LiquidityProviders, Pool, SyncConfig};
use std::str::FromStr;
use ethers::types::U256;
use ethers_providers::Http;
use garb_graph_eth::{GraphConfig, Order};
static PROVIDERS: Lazy<Vec<LiquidityProviders>> = Lazy::new(|| {
    std::env::var("ETH_PROVIDERS").unwrap_or_else(|_| std::env::args().nth(5).unwrap_or("1,2".to_string()))
                                    .split(",")
                                    .map(|s| s.parse::<u8>().unwrap())
                                    .map(|i| LiquidityProviders::from(i))
                                    .collect()
});

static CONTRACT_ADDRESS: Lazy<Address> = Lazy::new(|| {
    Address::from_str(&std::env::var("ETH_CONTRACT_ADDRESS").unwrap_or_else(|_| std::env::args().nth(6).unwrap_or("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419".to_string()))).unwrap()
});

static NODE_URL: Lazy<Url> = Lazy::new(|| {
    let url = std::env::var("ETH_NODE_URL").unwrap_or_else(|_| std::env::args().nth(7).unwrap_or("http://89.58.31.215:8545".to_string()));
    Url::parse(&url).unwrap()
});

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = Runtime::new()?;
    rt.block_on(async_main())?;
    Ok(())
}

pub async fn async_main() -> anyhow::Result<()> {
    let pools = Arc::new(RwLock::new(HashMap::<String, Pool>::new()));
    let (update_q_sender, update_q_receiver) = kanal::bounded_async::<Box<dyn EventSource<Event = Pool>>>(1000);
    // routes holds the routes that pass through an updated pool
    // this will be populated by the graph module when there is an updated pool
    let (routes_sender, mut routes_receiver) =
          kanal::bounded_async::<Order>(10000);
    
    let sync_config = SyncConfig {
        providers: PROVIDERS.clone(),
    };
    garb_sync_eth::start(pools.clone(), update_q_sender, sync_config)
          .await
          .unwrap();
    
    let mut joins = vec![];
    
    let graph_conifg = GraphConfig {
        from_file: false,
        save_only: false
    };
    let graph_routes = routes_sender.clone();
    joins.push(std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        
        rt.block_on(async move {
            
            garb_graph_eth::start(pools.clone(), update_q_receiver, Arc::new(RwLock::new(graph_routes)), graph_conifg).await;
            
        });
    }));
    joins.push(std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            transactor(&mut routes_receiver, routes_sender).await;
            
        });
    }));
  
    for join in joins {
        join.join().unwrap()
    }
    Ok(())
}

pub async fn transactor(routes: &mut kanal::AsyncReceiver<Order>, routes_sender: kanal::AsyncSender<Order>) {
    
    while let Ok(order) = routes.try_recv() {
        
        if let Some(order) = order {
            println!("`````````````````````` Tried Route ``````````````````````");
            for (i,pool) in order.route.iter().enumerate() {
                println!("{}. {}", i + 1, pool);
            }
            println!("\n\n");
            
            let mut directions = vec![];
            let mut pools = vec![];
            let mut pool_ids = vec![];
            
            for pool in order.route.iter() {
                directions.push(pool.x_to_y);
                pools.push(pool.address.clone().parse::<H160>().unwrap());
                pool_ids.push(pool.provider as u8);
            }
            
            let client = Arc::new(ethers_providers::Provider::<Http>::try_from(NODE_URL.clone().to_string()).unwrap());
            let contract = Aggregator::new(CONTRACT_ADDRESS.clone(), client.clone());
            
            contract.try_route(pools, pool_ids, directions, U256::from(order.size)).await.unwrap();
        }
       
    }
    
    
}