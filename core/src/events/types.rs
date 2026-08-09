//! 事件类型定义（事件驱动触发，T1 路线）。

use serde_json::Value;
use std::path::PathBuf;

/// 事件驱动触发的事件类型。
///
/// 文件系统监听（[`super::FileWatcher`]）与外部 webhook（`POST /api/v1/events`）
/// 统一进入事件总线，供规则匹配后触发任务或唤醒会话（ADR-013 通路）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 文件被创建（含重命名目标端）。
    FileCreated {
        /// 文件路径。
        path: PathBuf,
    },
    /// 文件内容被修改。
    FileModified {
        /// 文件路径。
        path: PathBuf,
    },
    /// 文件被删除（含重命名源端）。
    FileRemoved {
        /// 文件路径。
        path: PathBuf,
    },
    /// 外部 webhook 接入。
    Webhook {
        /// webhook 名称（规则 pattern `webhook:<名称>` 精确匹配）。
        name: String,
        /// webhook 载荷（任意 JSON）。
        payload: Value,
    },
}
