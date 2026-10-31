//! 设置页「数据存储」栏目：存储字段（字段引擎）+ 数据目录搬迁。
//!
//! 搬迁语义（ADR-023）：`POST /config/migrate-data-dir` 只**写入搬迁请求**并触发
//! 服务优雅关停；真正的「复制 → 校验 → 改配置 → 删源」发生在服务停掉之后（那时
//! SQLite / LanceDB 的文件锁才释放）。桌面版（Tauri）由监督循环自动执行并重启；
//! 原生端没有监督循环，所以由**下一次启动**在起 server 之前执行
//! （`main.rs` 里的 `run_pending_migration`）。UI 必须把这个前提说清楚，否则用户
//! 会以为点完没反应。

use crate::theme::colors;
use crate::types::*;
use crate::*;
// 字段表里的点路径取值（与字段引擎同一份实现）
use super::fields;
use crate::widgets::field_wrap;

impl ClientApp {
    /// 渲染「数据存储」栏目：字段卡片 + 搬迁块。
    pub(crate) fn render_settings_storage(&self, cx: &mut Context<Self>) -> AnyElement {
        let extra = self.render_storage_migration(cx);
        // 搬迁卡片紧跟「数据目录」一组字段：它是该字段**唯一**的修改入口
        self.render_settings_fields(SettingsTab::Storage, Some(extra), Some("数据目录"), cx)
    }

    /// 搬迁块（当前目录 / 目标目录输入 / 执行 / 前提说明）。
    fn render_storage_migration(&self, cx: &mut Context<Self>) -> AnyElement {
        // 目标目录为空时把按钮做成「禁用态」：搬错目录的代价是数据搬走 / 半途失败，
        // 不该让「没填就点」也走一趟后端校验。
        let target_ready = !self.storage_move_target.read(cx).value().trim().is_empty();
        let current = self
            .raw_config
            .as_ref()
            .and_then(|c| fields::json_get(c, "storage.data_dir"))
            .and_then(|v| v.as_str())
            .unwrap_or("（未加载）")
            .to_string();
        let mut card = div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child("搬迁数据目录"),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(format!("当前：{current}")),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(
                        "执行顺序：复制到新目录 → 校验 → 改 tianyan.toml → 删源（失败自动回滚）。\
                         服务会**先停止**，因此需要**关闭并重新打开本客户端**才能完成搬迁；\
                         重开后自动用新目录启动。",
                    ),
            )
            .child(field_wrap(&self.storage_move_target))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("storage-migrate")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(if target_ready {
                                colors::ACCENT()
                            } else {
                                colors::BG_TERTIARY()
                            }))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(if target_ready {
                                colors::TEXT_PRIMARY()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(if target_ready {
                                "搬迁到上面的目录"
                            } else {
                                "先填目标目录"
                            })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.migrate_data_dir(cx);
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("目标目录必须为空或不存在；不可位于当前数据目录内部"),
                    ),
            );
        if !self.storage_msg.is_empty() {
            card = card.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(if self.storage_msg_err {
                        colors::ERROR()
                    } else {
                        colors::SUCCESS()
                    }))
                    .child(self.storage_msg.clone()),
            );
        }
        card.into_any_element()
    }

    /// 发起数据目录搬迁（`POST /config/migrate-data-dir`）。
    pub(crate) fn migrate_data_dir(&mut self, cx: &mut Context<Self>) {
        let target = self.storage_move_target.read(cx).value().trim().to_string();
        if target.is_empty() {
            self.storage_msg = "请先填写目标目录（绝对路径）".to_string();
            self.storage_msg_err = true;
            cx.notify();
            return;
        }
        // 前置校验：只挡「一眼不对」的输入，让用户当场看到问题。真正的判据
        // （目标必须为空或不存在、不可嵌在当前目录内部）仍以后端为准 ——
        // 前端拿到的是原始字符串，规范化 / 盘符大小写 / 软链接都可能不同，
        // 两边各判一遍必然分叉，最后以后端那次为准。
        let current = self
            .raw_config
            .as_ref()
            .and_then(|c| fields::json_get(c, "storage.data_dir"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !std::path::Path::new(&target).is_absolute() {
            self.storage_msg = "目标目录必须是绝对路径（例如 D:\\tianyan-data）".to_string();
            self.storage_msg_err = true;
            cx.notify();
            return;
        }
        if !current.is_empty() && crate::same_dir(&target, &current) {
            self.storage_msg = "目标目录与当前数据目录是同一处，无需搬迁".to_string();
            self.storage_msg_err = true;
            cx.notify();
            return;
        }
        let port = self.port;
        self.storage_msg = "正在发起搬迁…".to_string();
        self.storage_msg_err = false;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/migrate-data-dir");
            let res = post_json(url, serde_json::json!({ "new_dir": target })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.storage_msg =
                            "搬迁已发起：服务即将停止。请关闭并重新打开客户端完成搬迁（重开后用新目录启动）"
                                .to_string();
                        this.storage_msg_err = false;
                    }
                    Err(e) => {
                        this.storage_msg = format!("搬迁未发起：{e}");
                        this.storage_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
