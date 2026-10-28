//! 知识页：浏览 / 导入 / 搜索三个 tab。
//!
//! 导入的文件选择是自绘弹窗（与 @ 引用共用一套浏览 UI）。

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::text::TextView;
use crate::types::*;
use crate::widgets::*;
use crate::format::*;

impl ClientApp {
    /// 记忆页：左列表（按目录分组）+ 右详情（L0/L1/L2 三层，L2 默认收起）。
    ///
    /// 不建完整树（Web 版 `buildMemoryTree`）：记忆条目本就按目录扁平分布，
    /// 一层分组标题 + 平铺足够定位，且省掉树节点的展开态管理。
    /// 知识页：三 tab（浏览 / 导入 / 搜索）。
    pub(crate) fn render_knowledge(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = div().flex().flex_row().items_center().gap_1();
        for (i, label) in ["浏览", "导入", "搜索"].iter().enumerate() {
            let idx = i as u8;
            let is_cur = self.kb_tab == idx;
            tabs = tabs.child(
                div()
                    .id(("kb-tab", i))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::BORDER()
                    }))
                    .bg(rgb(if is_cur {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.kb_tab = idx;
                        // 首次进导入 tab 才拉一次浏览目录（避免每次切都发请求）
                        if idx == 1 && this.kb_browse_entries.is_empty() {
                            this.load_kb_browse(String::new(), cx);
                        }
                        cx.notify();
                    })),
            );
        }

        let content = match self.kb_tab {
            1 => self.render_kb_ingest(cx).into_any_element(),
            2 => self.render_kb_search_tab(cx).into_any_element(),
            _ => self.render_kb_browse(cx).into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::BookOpen.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("知识库"),
                                    ),
                            )
                            .child(tabs),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(self.kb_status.clone()),
                    ),
            )
            .child(content)
    }

    /// 知识页·浏览：面包屑 + 条目列表 + 右内容（L0/L1/L2 切换）。
    pub(crate) fn render_kb_browse(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut crumbs = div().flex().flex_row().items_center().gap_1().child(
            div()
                .id("kb-root")
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("知识库根")
                .on_click(cx.listener(|this, _, _, cx| this.load_kb_entries(String::new(), cx))),
        );
        if !self.kb_path.is_empty() {
            let parent = self
                .kb_path
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default();
            crumbs = crumbs
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("/ {}", self.kb_path)),
                )
                .child(
                    div()
                        .id("kb-up")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(10.))
                        .text_color(rgb(colors::ACCENT()))
                        .child("↑ 上级")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_kb_entries(parent.clone(), cx)
                        })),
                );
        }

        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if self.kb_entries.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("这个目录是空的（或尚未导入任何文档）"),
            );
        }
        for (ix, e) in self.kb_entries.iter().enumerate() {
            let is_sel = self.kb_selected.as_deref() == Some(e.uri.as_str());
            let uri = e.uri.clone();
            let is_dir = e.is_dir;
            let layers = format!(
                "{}{}{}",
                if e.has_abstract { "L0 " } else { "" },
                if e.has_overview { "L1 " } else { "" },
                if e.has_detail { "L2" } else { "" }
            );
            list = list.child(
                div()
                    .id(("kb", ix))
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(if is_sel {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(rgb(if is_sel {
                                        colors::ACCENT()
                                    } else {
                                        colors::TEXT_PRIMARY()
                                    }))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(format!(
                                        "{}{}",
                                        if is_dir { "[D] " } else { "" },
                                        e.name
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(layers),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            // 目录：uri → 相对知识根的路径（`tianyan://knowledge/a/b` → `a/b`）
                            let rel = uri
                                .split("knowledge/")
                                .nth(1)
                                .unwrap_or_default()
                                .to_string();
                            this.load_kb_entries(rel, cx);
                        } else {
                            this.kb_selected = Some(uri.clone());
                            this.read_kb_entry(uri.clone(), "detail".to_string(), cx);
                        }
                    })),
            );
        }

        let detail = match (&self.kb_selected, &self.kb_content) {
            (Some(uri), Some((level, text))) => {
                let uri_owned = uri.clone();
                let uri_del = uri.clone();
                let mut levels = div().flex().flex_row().items_center().gap_1();
                for (li, lv) in ["abstract", "overview", "detail"].iter().enumerate() {
                    let is_cur = level == lv;
                    let u = uri_owned.clone();
                    levels = levels.child(
                        div()
                            .id(("kb-lv", li))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(if is_cur {
                                colors::ACCENT()
                            } else {
                                colors::BORDER()
                            }))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(if is_cur {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_SECONDARY()
                            }))
                            .child(lv.to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.read_kb_entry(u.clone(), lv.to_string(), cx)
                            })),
                    );
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_SECONDARY()))
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .overflow_hidden()
                                            .child(uri_owned.clone()),
                                    )
                                    .child(
                                        div()
                                            .id("kb-del")
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(rgb(colors::ERROR()))
                                            .cursor_pointer()
                                            .text_size(px(10.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("删除")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.delete_kb_entry(uri_del.clone(), cx)
                                            })),
                                    ),
                            )
                            .child(levels),
                    )
                    .child(
                        div().flex_1().min_h_0().p_4().child(
                            div()
                                .w_full()
                                .child(TextView::markdown(
                                    ("kb-md", 0usize),
                                    SharedString::from(text.clone()),
                                ))
                                .overflow_y_scrollbar(),
                        ),
                    )
                    .into_any_element()
            }
            _ => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::BookOpen.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选一个条目查看内容（目录点进去）"),
                )
                .into_any_element(),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().px_4().py_2().child(crumbs))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(320.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
    }

    /// 知识页·导入：选文件（弹窗）或按路径批量，读本地文件 → multipart 上传。
    ///
    /// 文件选择走**弹窗**（`Dialog::KbFilePicker`）而不是常驻列表：导入页的主任务只有
    /// 「选文件 + 导入」，把一张文件列表铺在页面上会把主次关系拉平、也挤掉结果区。
    pub(crate) fn render_kb_ingest(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 已选摘要：弹窗关掉后仍要看得到「将要导入什么」
        let picked_line = if self.kb_picked.is_empty() {
            "还没有选中文件".to_string()
        } else {
            format!(
                "已选 {} 个：{}",
                self.kb_picked.len(),
                self.kb_picked
                    .iter()
                    .take(4)
                    .map(|p| std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        let mut results = div().flex().flex_col().gap_1().w_full();
        for (i, (name, status)) in self.kb_ingest_results.iter().enumerate() {
            results = results.child(
                div()
                    .id(("kb-ir", i))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(status.clone()),
                    ),
            );
        }

        div()
            .flex_1()
            .min_h_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            // ① 主路径：选文件（弹窗）
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                svg()
                                    .path(IconName::Upload.path())
                                    .size(px(20.))
                                    .text_color(rgb(colors::ACCENT())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("选择文件导入"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(
                                                "可多选；导入是异步任务，L0/L1 摘要生成前浏览里可能还看不到",
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("kb-open-picker")
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT()))
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child("选择文件…")
                                    .on_click(cx.listener(|this, _, _, cx| this.open_kb_picker(cx))),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(picked_line),
                    )
                    .child(field_wrap(&self.kb_ingest_category)),
            )
            // ② 次路径：按路径批量（目录递归）
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("或按路径批量导入（文件或目录，目录递归，上限 200）"),
                    )
                    .child(field_wrap(&self.kb_ingest_path))
                    .child(
                        div()
                            .id("kb-ingest")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("按路径导入")
                            .on_click(cx.listener(|this, _, _, cx| this.ingest_kb(cx))),
                    ),
            )
            // ③ 结果
            .child(results)
            .overflow_y_scrollbar()
    }

    pub(crate) fn render_kb_search_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut results = div().flex().flex_col().gap_2().w_full();
        if self.kb_results.is_empty() {
            results = results.child(
                div()
                    .py_6()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("没有结果——输入词后点搜索"),
            );
        }
        for (i, r) in self.kb_results.iter().enumerate() {
            results = results.child(
                div()
                    .id(("kb-r", i))
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(r.source.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::ACCENT()))
                                    .child(format!("{:.3}", r.score)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(ellipsis(&r.content.replace('\n', " "), 300)),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(field_wrap(&self.kb_search)))
                    .child(
                        div()
                            .id("kb-do-search")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(colors::ACCENT()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("搜索")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let q = this.kb_search.read(cx).value().to_string();
                                this.search_kb(q, cx);
                            })),
                    ),
            )
            .child(results.overflow_y_scrollbar())
    }

    /// 知识页·浏览：拉某一级条目（`GET /knowledge/entries?path=`）。
    pub(crate) fn load_kb_entries(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let mut url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/entries");
            if !path.is_empty() {
                url.push_str(&format!("?path={}", url_encode(&path)));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<KbEntryRow> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(KbEntryRow {
                                        uri: e.get("uri")?.as_str()?.to_string(),
                                        name: e
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        is_dir: e
                                            .get("is_directory")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_abstract: e
                                            .get("has_abstract")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_overview: e
                                            .get("has_overview")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        has_detail: e
                                            .get("has_detail")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_path = path;
                        this.kb_entries = rows;
                        this.kb_status = format!("{n} 个条目");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("加载知识条目失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·浏览：读条目某一层内容（`GET /knowledge/entries/read`）。
    pub(crate) fn read_kb_entry(&self, uri: String, level: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/knowledge/entries/read?uri={}&level={}",
                url_encode(&uri),
                url_encode(&level)
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let got_level = body
                        .get("level")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&level)
                        .to_string();
                    let content = body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_selected = Some(uri);
                        this.kb_content = Some((got_level, content));
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("读取条目失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·浏览：删除条目（`POST /knowledge/entries/delete`；仅 knowledge 命名空间）。
    pub(crate) fn delete_kb_entry(&mut self, uri: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/entries/delete");
            let res = post_json(url, serde_json::json!({ "uri": uri })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.kb_status = "已删除".to_string();
                        this.kb_selected = None;
                        this.kb_content = None;
                        this.load_kb_entries(this.kb_path.clone(), cx);
                    }
                    Err(e) => this.kb_status = format!("删除失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 知识页·搜索：语义检索（`GET /knowledge/search?q=`）。
    pub(crate) fn search_kb(&self, q: String, cx: &mut Context<Self>) {
        if q.trim().is_empty() {
            return;
        }
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/knowledge/search?q={}&limit=20",
                url_encode(&q)
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<KbSearchRow> = body
                        .get("results")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some(KbSearchRow {
                                        id: r.get("id")?.as_str()?.to_string(),
                                        content: r
                                            .get("content")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        source: r
                                            .get("source")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        score: r
                                            .get("score")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0)
                                            as f32,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let total = body.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                    let _ = this.update(cx, |this, cx| {
                        this.kb_results = rows;
                        this.kb_status = format!("命中 {total} 条");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_status = format!("检索失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 知识页·导入：读本地路径 → multipart 上传（`POST /knowledge/ingest`）。
    ///
    /// 用**路径输入**而不是系统文件选择器：与 workspace 目录选择器同一手感，
    /// 且能被自动化 API 直接驱动（文件选择器是模态的，脚本没法点）。
    /// 目录会递归收集（上限 200 个文件——防手滑把整个盘导进去）。
    pub(crate) fn ingest_kb(&mut self, cx: &mut Context<Self>) {
        let raw = self.kb_ingest_path.read(cx).value().trim().to_string();
        let category = self.kb_ingest_category.read(cx).value().trim().to_string();
        self.kb_ingest_direct(raw, category, cx);
    }

    /// 路径输入入口：单一路径 → 交给 [`ClientApp::kb_ingest_paths`]。
    ///
    /// 自动化 `kb_ingest` 与手输路径共用这一条；文件选择器直接走 `kb_ingest_paths`。
    pub(crate) fn kb_ingest_direct(&mut self, raw: String, category: String, cx: &mut Context<Self>) {
        if raw.is_empty() {
            self.kb_status = "请选择文件/目录，或填写绝对路径".to_string();
            cx.notify();
            return;
        }
        self.kb_ingest_paths(vec![std::path::PathBuf::from(&raw)], category, cx);
    }

    /// 按路径列表导入（**路径输入与文件选择器共用这一份**）。
    pub(crate) fn kb_ingest_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        category: String,
        cx: &mut Context<Self>,
    ) {
        let files = collect_ingest_files(&paths);
        if files.is_empty() {
            self.kb_status = "选中的路径里没有可导入的文件".to_string();
            cx.notify();
            return;
        }
        let port = self.port;
        self.kb_status = format!("正在导入 {} 个文件…", files.len());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut form = reqwest::multipart::Form::new();
            if !category.is_empty() {
                form = form.text(
                    "metadata",
                    serde_json::json!({ "source_type": category }).to_string(),
                );
            }
            for f in &files {
                // 一次性导入动作，文件不大；此处阻塞读可接受（在 tokio 线程上顺序读）
                let Ok(bytes) = std::fs::read(f) else {
                    continue;
                };
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                form = form.part(
                    "files",
                    reqwest::multipart::Part::bytes(bytes).file_name(name),
                );
            }
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/knowledge/ingest");
            let res = reqwest::Client::new()
                .post(&url)
                .multipart(form)
                .send()
                .await;
            let parsed = match res {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(body) => Ok(body),
                    Err(e) => Err(format!("响应解析失败：{e}")),
                },
                Err(e) => Err(format!("请求失败：{e}")),
            };
            let _ = this.update(cx, |this, cx| {
                match parsed {
                    Ok(body) => {
                        let msg = body
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("已提交")
                            .to_string();
                        this.kb_ingest_results = body
                            .get("files")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|f| {
                                        Some((
                                            f.get("filename")?.as_str()?.to_string(),
                                            f.get("status")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("?")
                                                .to_string(),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        this.kb_status = msg;
                        this.load_kb_entries(String::new(), cx);
                    }
                    Err(e) => this.kb_status = e,
                }
                cx.notify();
            });
        })
        .detach();
    }
}
