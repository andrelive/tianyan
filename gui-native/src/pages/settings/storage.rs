//! 设置页「数据存储」栏目：字段引擎（数据目录只读）+ 搬迁入口。
//!
//! 搬迁语义（ADR-023）：`POST /config/migrate-data-dir` 只**写入搬迁请求**并触发
//! 服务优雅关停；真正的「复制 → 校验 → 改配置 → 删源」发生在服务停掉之后（那时
//! SQLite / LanceDB 的文件锁才释放）。原生端没有监督循环，所以由**下一次启动**
//! 在起 server 之前执行（`main.rs` 里的 `run_pending_migration`）。UI 必须把这个
//! 前提说清楚，否则用户会以为点完没反应。
//!
//! 入口只有一处：「数据目录」字段行右侧的「搬迁…」按钮 → 目录选择器 → 确认框
//! （`Dialog::DataDirMigrate`）。这里曾经是一张常驻卡片（当前目录 + 路径输入框 +
//! 执行按钮），三个问题：占掉半屏、手输路径没有浏览能力、和字段行的「数据目录」
//! 重复表达同一件事。搬迁是「这一个字段的动作」，就该长在这个字段上。

use super::fields;
use crate::types::*;
use crate::*;

impl ClientApp {
    /// 渲染「数据存储」栏目：数据目录（只读 + 搬迁按钮）+ 其余可编辑字段。
    pub(crate) fn render_settings_storage(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_settings_fields(SettingsTab::Storage, cx)
    }

    /// 发起数据目录搬迁（`POST /config/migrate-data-dir`）。
    ///
    /// 入口是确认框的「开始搬迁」（`Dialog::DataDirMigrate`）：选择器选完目录只是
    /// 第一步，整库搬迁不可撤销，多一道确认。
    pub(crate) fn migrate_data_dir(&mut self, target: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let target = target.trim().to_string();
        if target.is_empty() {
            self.push_notice("搬迁目标目录不能为空", true);
            return;
        }
        // 前置校验：只挡「一眼不对」的输入，让用户当场看到问题。真正的判据
        // （目标必须为空或不存在、不可嵌在当前目录内部）仍以后端为准 ——
        // 前端拿到的是原始字符串，规范化 / 盘符大小写 / 软链接都可能不同，
        // 两边各判一遍必然分叉，最终裁决只能有一处。
        if !std::path::Path::new(&target).is_absolute() {
            self.push_notice("搬迁目标必须是绝对路径（例如 D:\\tianyan-data）", true);
            return;
        }
        let current = self
            .raw_config
            .as_ref()
            .and_then(|c| fields::json_get(c, "storage.data_dir"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !current.is_empty() && crate::same_dir(&target, &current) {
            self.push_notice("目标目录与当前数据目录是同一处，无需搬迁", true);
            return;
        }
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/migrate-data-dir");
            let res = post_json(url, serde_json::json!({ "new_dir": target })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => this.push_notice(
                        "搬迁已发起：服务即将停止。请关闭并重新打开客户端完成搬迁（重开后用新目录启动）",
                        false,
                    ),
                    Err(e) => this.push_notice(format!("搬迁未发起：{e}"), true),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
