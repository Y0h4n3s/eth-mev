use std::collections::HashMap;
use std::sync::{Mutex, Arc};
use ethers::providers::{Provider, Ws};
use ethers_providers::Middleware;
use tracing::{debug, info};

#[derive(Clone)]
pub struct NodeDispatcher {
    nodes: Arc<Mutex<HashMap<String, usize>>>
}

impl NodeDispatcher {
    pub async fn from_file(file: &str) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(file)?;
        let mut nodes = HashMap::new();
        for line in contents
            .trim()
            .split("\n").into_iter() {
            let provider = Provider::<Ws>::connect(line).await;
            if let Ok(provider) = provider {
                let res = provider.get_block_number().await;
                if let Ok(info) = res {
                    debug!("Connecting go node {} at block {}", line, info.to_string());
                    nodes.insert(line.to_string(), 0);
                } else {
                    info!("Skipping node {} due to error: {:?}", line, res.unwrap_err())
                }
            } else {
                info!("Skipping node {} due to error: {:?}", line, provider.unwrap_err())
            }

        }
        Ok(Self {
            nodes: Arc::new(Mutex::new(nodes))
        })

    }

    pub fn next_free(&self) -> String {
        let mut w = self.nodes.lock().unwrap();

        let mut min = usize::MAX;
        let mut min_url = "".to_string();
        for (url, used) in w.iter() {
            if *used <= min {
                min = *used;
                min_url = url.to_string();
            }
        }
        w.insert(min_url.clone(), min + 1);
        info!("Node: {}", min_url);
        min_url
    }
}