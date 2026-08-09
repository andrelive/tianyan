//! 文件系统监听器：把 notify 事件映射为领域事件发布到事件总线。

use std::path::{Path, PathBuf};

use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::common::error::{Result, TianyanError};

use super::bus::EventBus;
use super::types::Event;

/// 文件监听器（基于 notify crate，T1 路线）。
///
/// [`start`](Self::start) 注册所有监听目录后立即返回；notify 在独立线程
/// 持续派发事件，由本模块过滤（事件类型 + 目录/临时文件噪音）并发布到
/// [`EventBus`]。监听持续到进程退出（运行时销毁时内部保持 task 被自动中止）。
pub struct FileWatcher {
    /// 事件发布目标总线。
    bus: EventBus,
}

impl FileWatcher {
    /// 创建文件监听器。
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }

    /// 启动文件监听（独立 task 运行，立即返回）。
    ///
    /// # 参数
    /// - `dirs`: 监听目录（递归）。
    /// - `events_filter`: 允许的事件类型（create/write/remove/rename 字符串）；
    ///   空列表 = 全部放行。rename 会被映射为两条事件：源端 `FileRemoved` +
    ///   目标端 `FileCreated`。
    ///
    /// # Errors
    /// - watcher 初始化失败或目录不存在/不可监听时返回 `TianyanError::Custom`。
    pub async fn start(&self, dirs: Vec<PathBuf>, events_filter: Vec<String>) -> Result<()> {
        let bus = self.bus.clone();
        let filter = EventFilter::new(events_filter);
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
                Ok(ev) => {
                    if let Some(event) = filter.translate(ev) {
                        bus.publish(event);
                    }
                }
                Err(e) => tracing::debug!(error = %e, "文件监听事件派发失败"),
            })
            .map_err(|e| TianyanError::Custom(format!("events: 初始化文件监听失败：{e}")))?;

        for dir in &dirs {
            watcher.watch(dir, RecursiveMode::Recursive).map_err(|e| {
                TianyanError::Custom(format!("events: 监听目录失败（{}）：{e}", dir.display()))
            })?;
            tracing::info!("开始监听目录：{}", dir.display());
        }

        // 保持 watcher 存活：notify 在独立线程派发事件，watcher 对象被 Drop 即停止。
        tokio::spawn(async move {
            let _watcher = watcher;
            std::future::pending::<()>().await;
        });

        Ok(())
    }
}

/// notify 事件 → 领域事件翻译器（事件类型过滤 + 噪音过滤）。
struct EventFilter {
    /// 允许的事件类型（create/write/remove/rename 小写；空 = 全部放行）。
    allow: Vec<String>,
}

impl EventFilter {
    /// 创建过滤器。
    fn new(events_filter: Vec<String>) -> Self {
        let allow: Vec<String> = events_filter
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        Self { allow }
    }

    /// 是否允许指定事件类型。
    fn allows(&self, kind: &str) -> bool {
        self.allow.is_empty() || self.allow.iter().any(|a| a == kind)
    }

    /// 翻译单个 notify 事件；被过滤（类型不允/目录/临时文件噪音）时返回 None。
    fn translate(&self, ev: notify::Event) -> Option<Event> {
        let path = ev.paths.iter().find(|p| !is_noise(p))?;
        if path.is_dir() {
            return None;
        }
        match ev.kind {
            EventKind::Create(_) if self.allows("create") => {
                Some(Event::FileCreated { path: path.clone() })
            }
            EventKind::Modify(ModifyKind::Data(_)) | EventKind::Modify(ModifyKind::Any)
                if self.allows("write") =>
            {
                Some(Event::FileModified { path: path.clone() })
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::From))
                if self.allows("rename") || self.allows("remove") =>
            {
                Some(Event::FileRemoved { path: path.clone() })
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::To))
                if self.allows("rename") || self.allows("create") =>
            {
                Some(Event::FileCreated { path: path.clone() })
            }
            EventKind::Remove(_) if self.allows("remove") => {
                Some(Event::FileRemoved { path: path.clone() })
            }
            // 其余事件（Access / Metadata 变更 / Other）一律视为噪音
            _ => None,
        }
    }
}

/// 是否为临时/备份噪音文件。
///
/// 覆盖：`.tmp` / `.swp` / `.swo` / `.part` 扩展名、结尾 `~`（vi 备份）、
/// 开头 `.#`（Emacs 锁文件）——防止编辑器中间产物触发规则。
fn is_noise(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    if name.ends_with('~') || name.starts_with(".#") {
        return true;
    }
    let lower = name.to_lowercase();
    ["tmp", "swp", "swo", "part"]
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::bus::EventBus;
    use std::time::Duration;

    /// 带超时的接收（notify 异步派发，单次最长等待 5s）。
    async fn recv_with_timeout(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> Option<Event> {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn test_watcher_emits_created_and_modified() {
        let tempdir = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        FileWatcher::new(bus.clone())
            .start(
                vec![tempdir.path().to_path_buf()],
                vec!["create".to_string(), "write".to_string()],
            )
            .await
            .unwrap();
        let mut rx = bus.subscribe();

        // 创建文件 → FileCreated（跳过可能先到的其他事件）
        let file = tempdir.path().join("notes.md");
        std::fs::write(&file, "hello").unwrap();
        let created = loop {
            match recv_with_timeout(&mut rx).await {
                Some(e @ Event::FileCreated { .. }) => break e,
                Some(_) => continue,
                None => panic!("5s 内未收到 FileCreated"),
            }
        };
        assert_eq!(created, Event::FileCreated { path: file.clone() });

        // 修改文件 → FileModified
        std::fs::write(&file, "hello world").unwrap();
        let modified = loop {
            match recv_with_timeout(&mut rx).await {
                Some(e @ Event::FileModified { .. }) => break e,
                Some(_) => continue,
                None => panic!("5s 内未收到 FileModified"),
            }
        };
        assert_eq!(modified, Event::FileModified { path: file });
    }

    #[tokio::test]
    async fn test_watcher_filters_temp_files() {
        let tempdir = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        FileWatcher::new(bus.clone())
            .start(
                vec![tempdir.path().to_path_buf()],
                vec!["create".to_string()],
            )
            .await
            .unwrap();
        let mut rx = bus.subscribe();

        // 临时文件（.tmp）：不应产生事件
        let tmp = tempdir.path().join("scratch.tmp");
        std::fs::write(&tmp, "x").unwrap();
        // 正常文件：应产生事件
        let good = tempdir.path().join("real.txt");
        std::fs::write(&good, "x").unwrap();

        let event = recv_with_timeout(&mut rx)
            .await
            .expect("正常文件应触发事件");
        assert_eq!(event, Event::FileCreated { path: good });

        // 再等待 1s，确认没有 .tmp 事件混入
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            match rx.try_recv() {
                Ok(e) => panic!("临时文件不应触发事件：{e:?}"),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                _ => break,
            }
        }
    }

    #[test]
    fn test_translate_maps_notify_kinds() {
        let filter = EventFilter::new(vec![]);
        let created = notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
            .add_path(PathBuf::from("a.txt"));
        assert_eq!(
            filter.translate(created).unwrap(),
            Event::FileCreated {
                path: PathBuf::from("a.txt")
            }
        );

        let modified = notify::Event::new(EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path(PathBuf::from("a.txt"));
        assert_eq!(
            filter.translate(modified).unwrap(),
            Event::FileModified {
                path: PathBuf::from("a.txt")
            }
        );

        // Windows 上内容修改派发为 Modify(Any)：同样映射为 FileModified
        let modified_any =
            notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(PathBuf::from("a.txt"));
        assert_eq!(
            filter.translate(modified_any).unwrap(),
            Event::FileModified {
                path: PathBuf::from("a.txt")
            }
        );

        let removed = notify::Event::new(EventKind::Remove(notify::event::RemoveKind::File))
            .add_path(PathBuf::from("a.txt"));
        assert_eq!(
            filter.translate(removed).unwrap(),
            Event::FileRemoved {
                path: PathBuf::from("a.txt")
            }
        );

        // Access / Metadata 变更视为噪音
        let access = notify::Event::new(EventKind::Access(notify::event::AccessKind::Read))
            .add_path(PathBuf::from("a.txt"));
        assert!(filter.translate(access).is_none());

        let meta = notify::Event::new(EventKind::Modify(ModifyKind::Metadata(
            notify::event::MetadataKind::Any,
        )))
        .add_path(PathBuf::from("a.txt"));
        assert!(filter.translate(meta).is_none());
    }

    #[test]
    fn test_translate_respects_event_filter() {
        // 只允许 remove：create 被过滤
        let filter = EventFilter::new(vec!["remove".to_string()]);
        let created = notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
            .add_path(PathBuf::from("a.txt"));
        assert!(filter.translate(created).is_none());

        // rename 源端：需要 rename 或 remove
        let renamed_from =
            notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
                .add_path(PathBuf::from("old.txt"));
        assert_eq!(
            filter.translate(renamed_from).unwrap(),
            Event::FileRemoved {
                path: PathBuf::from("old.txt")
            }
        );

        // rename 目标端：需要 rename 或 create（当前过滤不允许）
        let renamed_to = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To)))
            .add_path(PathBuf::from("new.txt"));
        assert!(filter.translate(renamed_to).is_none());

        // 全部放行时 rename 目标端映射为 FileCreated
        let filter_all = EventFilter::new(vec![]);
        let renamed_to = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To)))
            .add_path(PathBuf::from("new.txt"));
        assert_eq!(
            filter_all.translate(renamed_to).unwrap(),
            Event::FileCreated {
                path: PathBuf::from("new.txt")
            }
        );
    }

    #[test]
    fn test_translate_filters_noise_and_dirs() {
        let filter = EventFilter::new(vec![]);
        for name in ["x.tmp", "x.swp", "x.swo", "x.part", "x~"] {
            let ev = notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
                .add_path(PathBuf::from(name));
            assert!(filter.translate(ev).is_none(), "{name} 应为噪音");
        }

        // 目录事件（真实存在的目录）被过滤
        let dir = tempfile::tempdir().unwrap();
        let dir_ev = notify::Event::new(EventKind::Create(notify::event::CreateKind::Any))
            .add_path(dir.path().to_path_buf());
        assert!(filter.translate(dir_ev).is_none());
    }
}
