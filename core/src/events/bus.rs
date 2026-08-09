//! 事件总线（发布/订阅）。

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::types::Event;

/// 进程内事件总线。
///
/// 发布/订阅模型：[`subscribe`](Self::subscribe) 为调用方建立独立的无界通道
/// （tokio mpsc unbounded），[`publish`](Self::publish) 向所有存活订阅者广播。
/// [`EventBus`] 可 Clone（共享同一份订阅表），供 [`super::FileWatcher`]、
/// HTTP webhook 处理器与事件消费处理器之间传递。
#[derive(Clone)]
pub struct EventBus {
    /// 所有存活订阅者的发送端（发布时遍历；已关闭的订阅会被剔除）。
    subscribers: Arc<Mutex<Vec<UnboundedSender<Event>>>>,
}

impl EventBus {
    /// 创建空事件总线。
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 订阅事件流，返回独立的事件接收端。
    ///
    /// 接收端被 Drop 后自动从总线剔除，后续 [`publish`](Self::publish)
    /// 不再向它投递。
    pub fn subscribe(&self) -> UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.push(tx);
        rx
    }

    /// 发布事件到所有存活订阅者（非阻塞；订阅者已关闭时静默剔除）。
    pub fn publish(&self, event: Event) {
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_publish_and_subscribe() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.publish(Event::FileCreated {
            path: PathBuf::from("a.txt"),
        });

        let e1 = rx1.recv().await.expect("订阅者 1 应收到事件");
        let e2 = rx2.recv().await.expect("订阅者 2 应收到事件");
        assert_eq!(e1, e2);
        assert_eq!(
            e1,
            Event::FileCreated {
                path: PathBuf::from("a.txt")
            }
        );
    }

    #[tokio::test]
    async fn test_bus_clone_shares_subscribers() {
        let bus = EventBus::new();
        let bus2 = bus.clone();
        let mut rx = bus.subscribe();

        // 通过克隆发布，原订阅者仍能收到
        bus2.publish(Event::FileRemoved {
            path: PathBuf::from("a.txt"),
        });
        assert_eq!(
            rx.recv().await.expect("应收到事件"),
            Event::FileRemoved {
                path: PathBuf::from("a.txt")
            }
        );
    }

    #[tokio::test]
    async fn test_dropped_subscriber_is_pruned() {
        let bus = EventBus::new();
        {
            let _rx = bus.subscribe();
        }
        // 订阅者已 Drop：发布不 panic，且不再向它投递
        bus.publish(Event::FileCreated {
            path: PathBuf::from("a.txt"),
        });

        let mut rx = bus.subscribe();
        bus.publish(Event::FileCreated {
            path: PathBuf::from("b.txt"),
        });
        assert_eq!(
            rx.recv().await.expect("应收到事件"),
            Event::FileCreated {
                path: PathBuf::from("b.txt")
            }
        );
    }
}
