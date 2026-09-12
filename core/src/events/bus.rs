//! 事件总线（发布/订阅）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{self, error::TrySendError, Receiver, Sender};

use super::types::Event;

/// 订阅通道容量（有界背压）。
///
/// 无界通道在订阅者（事件处理器 / SSE 推送）消费慢时会无限积压内存，
/// 最终拖垮进程。有界 + `try_send` 把「无限积压」换成「可观测的丢弃」：
/// 被丢弃的事件只影响实时通知，权威数据仍在 SQLite / 命令日志文件中。
pub const EVENT_BUS_CAPACITY: usize = 4096;

/// 进程内事件总线。
///
/// 发布/订阅模型：[`subscribe`](Self::subscribe) 为调用方建立独立的有界通道
/// （容量 [`EVENT_BUS_CAPACITY`]），[`publish`](Self::publish) 向所有存活订阅者
/// 广播；订阅者积压时丢弃该事件并计数（`try_send`，绝不阻塞发布方）。
/// [`EventBus`] 可 Clone（共享同一份订阅表与丢弃计数），供
/// [`super::FileWatcher`]、HTTP webhook 处理器与事件消费处理器之间传递。
#[derive(Clone)]
pub struct EventBus {
    /// 所有存活订阅者的发送端（发布时遍历；已关闭的订阅会被剔除）。
    subscribers: Arc<Mutex<Vec<Sender<Event>>>>,
    /// 因订阅者积压而丢弃的事件累计数（背压观测）。
    dropped: Arc<AtomicU64>,
}

impl EventBus {
    /// 创建空事件总线。
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 订阅事件流，返回独立的事件接收端。
    ///
    /// 接收端被 Drop 后自动从总线剔除，后续 [`publish`](Self::publish)
    /// 不再向它投递。
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel(EVENT_BUS_CAPACITY);
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.push(tx);
        rx
    }

    /// 发布事件到所有存活订阅者（非阻塞）。
    ///
    /// 订阅者通道已满时**丢弃该事件**（不阻塞发布方、不无限积压），并累计到
    /// [`dropped_events`](Self::dropped_events)；订阅者已关闭时静默剔除。
    pub fn publish(&self, event: Event) {
        let mut subscribers = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        subscribers.retain(|tx| match tx.try_send(event.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                // 只在首次与每 100 次打点：积压期间避免日志风暴
                if total == 1 || total.is_multiple_of(100) {
                    tracing::warn!(
                        dropped_total = total,
                        capacity = EVENT_BUS_CAPACITY,
                        "事件总线订阅者积压：事件被丢弃（实时通知尽力而为，权威数据不受影响）"
                    );
                }
                true
            }
            Err(TrySendError::Closed(_)) => false,
        });
    }

    /// 累计被丢弃的事件数（背压观测：>0 说明有订阅者消费不及）。
    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
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

    /// 回归测试：订阅者不消费时发布**有界背压**——超过容量的事件被丢弃并
    /// 计数，而不是无限积压（旧的无界通道实现下 `dropped_events()` 恒为 0）。
    #[tokio::test]
    async fn test_bounded_backpressure_drops_when_subscriber_stalls() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let overflow = 50;
        for i in 0..(EVENT_BUS_CAPACITY + overflow) {
            bus.publish(Event::FileCreated {
                path: PathBuf::from(format!("f{i}.txt")),
            });
        }

        assert!(
            bus.dropped_events() >= overflow as u64,
            "超过容量的事件应被丢弃并计数（实际 {}）",
            bus.dropped_events()
        );
        // 丢弃发生在新事件上：缓冲内已入队的首个事件仍可读取
        assert_eq!(
            rx.recv().await.expect("应收到首个事件"),
            Event::FileCreated {
                path: PathBuf::from("f0.txt")
            }
        );
    }
}
