use async_std::sync::Arc;
use async_trait::async_trait;
use kanal::{AsyncReceiver, AsyncSender};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

#[async_trait]
pub trait EventSink {
    type EventType: Send;
    fn get_receiver(&self) -> Arc<AsyncReceiver<Self::EventType>>;
    fn handle_event(&self, event_msg: Self::EventType) -> JoinHandle<()>;
    async fn listen(&self) -> JoinHandle<()> {
        let receiver = self.get_receiver();
        while let Ok(event) = receiver.recv().await {
            self.handle_event(event).await;
        }
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
        })
    }
}

#[async_trait]
pub trait EventEmitter {
    type EventType: Send;
    fn get_subscribers(&self) -> Arc<RwLock<Vec<AsyncSender<Self::EventType>>>>;
    async fn subscribe(&mut self, sender: AsyncSender<Self::EventType>) {
        let subs = self.get_subscribers();
        subs.write().await.push(sender);
    }
    fn emit(&self) -> std::thread::JoinHandle<()>;
}
