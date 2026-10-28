//! 文件页：工作区树 + 文件内容 + diff 面板。
//!
//! 目录树与文件内容都走 `/workspace/*` 接口；diff 面板是「改动清单 +
//! 单文件 unified diff」，数据来自快照系统的 `build_hunks`。

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use crate::types::*;

impl ClientApp {
    /// 设置页（第一版）：模型列表 + 切换聊天模型。
    ///
    /// 只做「能日常用」的最小集合——没有它，换模型只能回 Tauri 版改配置。
    /// 存储 / 安全 / MCP / soul 等分组按后续批次补（端点都已存在，见
    /// `server/src/api/config/routes.rs`）。
    /// 子智能体页（对齐 `gui-vite/src/components/roles/RolesPanel.tsx`）：
    /// 左列表（搜索 + 角色项）+ 右详情（统计条 + 系统提示 / 工具 / 模型）。
    /// 文件页（对齐 Web 版 `WorkspacePanel` 的**只读部分**）：左侧单层目录树 +
    /// 右侧文件内容。
    ///
    /// 编辑保存（apply-patch）、diff 面板、目录绑定留阶段 2 —— 那三块各自要一套
    /// 状态机（草稿 / 冲突 / 快照索引），先把"能看"落地。
    pub(crate) fn render_workspace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let crumb = if self.ws_path.is_empty() {
            if self.ws_root.is_empty() {
                "未选择会话（无可浏览目录）".to_string()
            } else {
                self.ws_root.clone()
            }
        } else {
            format!(
                "{}/{}",
                self.ws_root.trim_end_matches(['\\', '/']),
                self.ws_path
            )
        };

        let mut list = div().flex_1().min_h_0().w_full().flex().flex_col().gap_1();
        if !self.ws_path.is_empty() {
            let parent = {
                let p = self.ws_path.replace('\\', "/");
                match p.rfind('/') {
                    Some(pos) => p[..pos].to_string(),
                    None => String::new(),
                }
            };
            list = list.child(
                div()
                    .id("ws-up")
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(
                        svg()
                            .path(IconName::CornerLeftUp.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child("上级目录")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_workspace_tree(parent.clone(), cx);
                    })),
            );
        }
        for (ix, entry) in self.ws_entries.iter().enumerate() {
            let is_dir = entry.is_dir;
            let path = entry.path.clone();
            let name = entry.name.clone();
            let size = entry.size;
            list = list.child(
                div()
                    .id(("ws-entry", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        svg()
                            .path(if is_dir {
                                IconName::Folder.path()
                            } else {
                                IconName::FileText.path()
                            })
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(if is_dir {
                                colors::TEXT_PRIMARY()
                            } else {
                                colors::TEXT_SECONDARY()
                            }))
                            .child(name),
                    )
                    .children(if is_dir {
                        None
                    } else {
                        Some(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(format_size(size)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            this.load_workspace_tree(path.clone(), cx);
                        } else {
                            this.load_workspace_file(path.clone(), cx);
                        }
                    })),
            );
        }

        let content = match &self.ws_file {
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    svg()
                        .path(IconName::FileText.path())
                        .size(px(36.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("从左侧选择一个文件查看内容"),
                )
                .into_any_element(),
            Some((path, text)) => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER()))
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(path.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_4()
                        .text_size(px(12.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(text.clone())
                        .overflow_y_scrollbar(),
                )
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        svg()
                            .path(IconName::FolderTree.path())
                            .size(px(16.))
                            .text_color(rgb(colors::TEXT_SECONDARY())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(15.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("文件"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(crumb),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(self.ws_status.clone()),
                    )
                    // 选择工作目录：逐级浏览后绑定到当前会话
                    .child(
                        div()
                            .id("ws-pick-dir")
                            .flex_shrink_0()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::FolderOpen.path())
                                    .size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .child("选择目录")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_workspace_picker(PickerTarget::Session, cx);
                            })),
                    )
                    // Diff 开关：右侧展开「工作区改动」面板（对照会话快照基线）
                    .child(
                        div()
                            .id("ws-diff-toggle")
                            .flex_shrink_0()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(if self.ws_diff_open {
                                colors::ACCENT()
                            } else {
                                colors::BORDER()
                            }))
                            .text_size(px(11.))
                            .text_color(rgb(if self.ws_diff_open {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_SECONDARY()
                            }))
                            .cursor_pointer()
                            .child("Diff")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ws_diff_open = !this.ws_diff_open;
                                if this.ws_diff_open {
                                    this.load_workspace_diff_list(cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(280.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(if self.ws_path.is_empty() {
                                        "工作区根目录".to_string()
                                    } else {
                                        self.ws_path.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p_2()
                                    .child(list.overflow_y_scrollbar()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(rgb(colors::BG_PRIMARY()))
                            .child(content),
                    )
                    // diff 面板（右侧，可开关）：AI 改了什么，一眼看到
                    .children(if self.ws_diff_open {
                        Some(self.render_diff_panel(cx))
                    } else {
                        None
                    }),
            )
    }

    /// diff 面板（右侧）：改动文件列表 + 选中文件的 unified diff（+/- 染色）。
    pub(crate) fn render_diff_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut files = div().flex().flex_col().gap_1().w_full();
        if self.ws_diff_files.is_empty() {
            files = files.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(self.ws_diff_status.clone()),
            );
        }
        for (ix, f) in self.ws_diff_files.iter().enumerate() {
            let selected = self
                .ws_diff
                .as_ref()
                .map(|(p, _)| p == &f.path)
                .unwrap_or(false);
            let (color, label) = match f.status.as_str() {
                "added" => (colors::SUCCESS(), "A"),
                "removed" => (colors::ERROR(), "D"),
                "binary" => (colors::WARNING(), "B"),
                _ => (colors::ACCENT(), "M"),
            };
            let path = f.path.clone();
            files = files.child(
                div()
                    .id(("ws-diff-file", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .bg(rgb(if selected {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(color))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(f.path.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("+{} −{}", f.new_lines, f.old_lines)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_workspace_diff(path.clone(), cx);
                    })),
            );
        }

        let detail = match &self.ws_diff {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选一个文件查看 diff"),
                )
                .into_any_element(),
            Some((_, unified)) => {
                let mut lines = div().flex().flex_col().w_full();
                for line in unified.lines().take(3000) {
                    let color = if line.starts_with("+++") || line.starts_with("---") {
                        colors::TEXT_TERTIARY()
                    } else if line.starts_with('+') {
                        colors::SUCCESS()
                    } else if line.starts_with('-') {
                        colors::ERROR()
                    } else if line.starts_with("@@") {
                        colors::ACCENT()
                    } else {
                        colors::TEXT_SECONDARY()
                    };
                    lines = lines.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(color))
                            .child(line.to_string()),
                    );
                }
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .p_2()
                    .child(lines.overflow_y_scrollbar())
                    .into_any_element()
            }
        };

        div()
            .w(px(380.))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child("工作区改动（相对会话起点快照）"),
            )
            // 列表必须自己可滚动：改动文件可能几十个，`max_h` 只限高不裁剪，
            // 内容会溢出盖到下面的 diff 区（实测二十多个文件时整片文字重叠）
            .child(
                div()
                    // 同 picker：滚动容器必须落在确定高度里（`max_h` 会让它塌成 0）
                    .h(px(240.))
                    .min_h_0()
                    .p_2()
                    .child(files.overflow_y_scrollbar()),
            )
            .child(detail)
            .into_any_element()
    }

    /// 加载工作区目录树（`GET /workspace/tree?depth=1&session_id=`）。
    ///
    /// 单层浏览（点目录进、点「上级」出）：树的可展开形态要维护每个节点的
    /// 独立状态与嵌套请求，先按单层落，够用且不易错。
    pub(crate) fn load_workspace_tree(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!("http://{SERVER_HOST}:{port}/api/v1/workspace/tree?depth=1");
            if !path.is_empty() {
                url.push_str(&format!("&path={}", url_encode(&path)));
            }
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let root = body
                        .get("root")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut entries: Vec<WsEntry> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(WsEntry {
                                        name: e.get("name")?.as_str()?.to_string(),
                                        is_dir: e.get("type").and_then(|v| v.as_str())
                                            == Some("dir"),
                                        path: e
                                            .get("path")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        size: e.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // 目录在前、文件在后，各自按名字排序（与常见文件浏览器一致）
                    entries.sort_by(|a, b| {
                        b.is_dir
                            .cmp(&a.is_dir)
                            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    });
                    let n = entries.len();
                    let _ = this.update(cx, |this, cx| {
                        this.ws_root = root;
                        this.ws_path = path;
                        this.ws_entries = entries;
                        this.ws_status = format!("{n} 项");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 读取工作区文件（`GET /workspace/read?limit=2000&session_id=`）。
    pub(crate) fn load_workspace_file(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/workspace/read?limit=2000&path={}",
                url_encode(&path)
            );
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let truncated = body
                        .get("truncated")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let total = body.get("total_lines").and_then(|v| v.as_u64());
                    let raw = body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let text = strip_hashline(raw);
                    let status = if raw.is_empty() {
                        "（空文件或二进制，无法以文本显示）".to_string()
                    } else if truncated {
                        format!(
                            "已截断：仅显示前 2000 行（共 {} 行）",
                            total
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".to_string())
                        )
                    } else {
                        format!(
                            "{} 行",
                            total
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| text.lines().count().to_string())
                        )
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.ws_file = Some((path, text));
                        this.ws_status = status;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_status = format!("读取文件失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 拉工作区改动文件列表（`GET /workspace/diff?base=snapshot`，不带 path）。
    ///
    /// 这是「AI 到底改了什么」的入口。**快照是按消息存的**（后端要求
    /// `message_id` 或 `index`，缺了直接 400）—— `index=0` = 会话起点那一刻的
    /// 工作区快照，也就是「本次会话从头到现在改了什么」。
    pub(crate) fn load_workspace_diff_list(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url =
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/diff?base=snapshot&index=0");
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let files: Vec<WsDiffFile> = body
                        .get("files")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|f| {
                                    Some(WsDiffFile {
                                        path: f.get("path")?.as_str()?.to_string(),
                                        status: f
                                            .get("status")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("modified")
                                            .to_string(),
                                        old_lines: f
                                            .get("old_lines")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        new_lines: f
                                            .get("new_lines")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = files.len();
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_files = files;
                        this.ws_diff_status = if n == 0 {
                            "工作区无改动（相对本次会话快照）".to_string()
                        } else {
                            format!("{n} 个文件有改动")
                        };
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_status = format!("读取改动列表失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 拉单个文件的 unified diff（`GET /workspace/diff?base=snapshot&index=0&path=`）。
    pub(crate) fn load_workspace_diff(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let mut url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/workspace/diff?base=snapshot&index=0&path={}",
                url_encode(&path)
            );
            if let Some(sid) = &session {
                url.push_str(&format!("&session_id={sid}"));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let unified = body
                        .get("unified")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let status = body
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("modified")
                        .to_string();
                    let text = if unified.is_empty() {
                        match status.as_str() {
                            "binary" => "（二进制文件，无文本 diff）".to_string(),
                            "unchanged" => "（无改动）".to_string(),
                            _ => "（服务端未返回 unified diff）".to_string(),
                        }
                    } else {
                        unified
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff = Some((path, text));
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ws_diff_status = format!("读取 diff 失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}
