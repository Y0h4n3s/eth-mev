use once_cell::sync::Lazy;
use tokio::runtime::Runtime;
use async_std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::{HashMap};
use garb_sync_eth::{EventSource, LiquidityProviders, Pool, SyncConfig};
use garb_graph_eth::{GraphConfig, Order};
static PROVIDERS: Lazy<Vec<LiquidityProviders>> = Lazy::new(|| {
    std::env::var("ETH_PROVIDERS").unwrap_or_else(|_| std::env::args().nth(5).unwrap_or("1,2".to_string()))
                                    .split(",")
                                    .map(|s| s.parse::<u8>().unwrap())
                                    .map(|i| LiquidityProviders::from(i))
                                    .collect()
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
  
    for join in joins {
        join.join().unwrap()
    }
    Ok(())
}