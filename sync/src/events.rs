#![allow(where_clauses_object_safety)]
use async_std::sync::Arc;
use async_trait::async_trait;
use kanal::{AsyncReceiver, AsyncSender};
use std::time::Duration;
use std::sync::RwLock;
use tokio::task::JoinHandle;

pub trait EventEmitter<T: Send> {
    fn get_subscribers(&self) -> Arc<RwLock<Vec<AsyncSender<T>>>>;
    fn subscribe(&mut self, sender: AsyncSender<T>) {
        let subs = self.get_subscribers();
        subs.write().unwrap().push(sender);
    }
    fn emit(&self) -> std::thread::JoinHandle<()>;
}
