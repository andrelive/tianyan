//! 审批页：工作流配置摘要 + 自动规则 + 审计记录。
//!

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::format::*;
use crate::theme::colors;
use crate::types::*;
use gpui_kit::assets::IconName;

impl ClientApp {
    /// 审批页：工作流配置 + 待审批（可操作）+ 待确认指纹 + 审计记录。
    ///
    /// 审批链路口径（ADR-033）：工具执行门控 → 风险分级 → 自动审批规则
    /// （Deny 优先）→ `ApprovalMode` 三态短路 → 人工审批挂起通道。
    /// 本页是这个链路的**观测面**：配置长什么样、当前挂起什么、过往怎么判的。
    pub(crate) fn render_approval(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        // 1. 配置卡
        let mut cfg = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("工作流配置"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_TERTIARY()))
                            .text_size(px(10.))
                            .text_color(rgb(colors::ACCENT()))
                            .child(if self.approval_mode.is_empty() {
                                "未装配".to_string()
                            } else {
                                format!("模式 {}", self.approval_mode)
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if self.approval_auto {
                                colors::SUCCESS()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(if self.approval_auto {
                                "自动审批已启用"
                            } else {
                                "自动审批已关闭"
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!(
                                "超时 {}s · 最大挂起 {} · 已确认指纹 {}",
                                self.approval_timeout,
                                self.approval_max_pending,
                                self.confirmed_action_count
                            )),
                    ),
            );
        if !self.approval_prompt_cmds.is_empty() {
            let mut chips = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for c in &self.approval_prompt_cmds {
                chips = chips.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::WARNING()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::WARNING()))
                        .child(c.clone()),
                );
            }
            cfg = cfg.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("总是询问的命令（不被自动规则与任何模式放行）"),
                    )
                    .child(chips),
            );
        }
        if !self.approval_rules.is_empty() {
            let mut rules = div().flex().flex_col().gap_1().w_full();
            for r in self.approval_rules.iter().take(12) {
                rules = rules.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .child(r.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(if r.decision.eq_ignore_ascii_case("deny") {
                                    colors::ERROR()
                                } else {
                                    colors::SUCCESS()
                                }))
                                .child(r.decision.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(if r.enabled { "启用" } else { "停用" }),
                        ),
                );
            }
            cfg = cfg.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!(
                                "自动审批规则（{} 条，Deny 优先）",
                                self.approval_rules.len()
                            )),
                    )
                    .child(rules),
            );
        }
        body = body.child(cfg);

        // 2. 待审批（可操作）
        let mut pending = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(if self.pending_approvals.is_empty() {
                colors::BORDER()
            } else {
                colors::WARNING()
            }))
            .bg(rgb(colors::BG_SECONDARY()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child(format!("待审批（{}）", self.pending_approvals.len())),
            );
        if self.pending_approvals.is_empty() {
            pending = pending.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("没有挂起的审批请求"),
            );
        }
        for (ix, a) in self.pending_approvals.iter().enumerate() {
            let rid_a = a.request_id.clone();
            let rid_d = a.request_id.clone();
            let rid_e = a.request_id.clone();
            let edit_cmd = a.action_detail.clone();
            pending = pending.child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_PRIMARY()))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child(a.description.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(risk_color(&a.risk)))
                                    .child(format!("风险 {}", a.risk)),
                            ),
                    )
                    // 具体操作原文（审批的意义就是「看清再点」）
                    .children(if a.action_detail.is_empty() {
                        None
                    } else {
                        Some(
                            div()
                                .w_full()
                                .rounded_md()
                                .bg(rgb(colors::BG_PRIMARY()))
                                .p_2()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child(ellipsis(&a.action_detail, 400)),
                        )
                    })
                    .child(
                        div()
                            .id(("ap-edit", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("编辑后批准")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_edit_approval(rid_e.clone(), edit_cmd.clone(), window, cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("ap-ok", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::SUCCESS()))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("允许")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_approval(rid_a.clone(), "approve", None, cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("ap-no", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::ERROR()))
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("拒绝")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond_approval(rid_d.clone(), "deny", None, cx)
                            })),
                    ),
            );
        }
        body = body.child(pending);

        // 3. 待确认指纹（「询问用户」降级链路）
        if !self.pending_confirmations.is_empty() {
            let mut list = div().flex().flex_col().gap_1().w_full();
            for (ix, fp) in self.pending_confirmations.iter().enumerate() {
                list = list.child(
                    div()
                        .id(("ap-fp", ix))
                        .w_full()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(fp.clone()),
                );
            }
            body = body.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(format!(
                                "待确认操作指纹（{}）",
                                self.pending_confirmations.len()
                            )),
                    )
                    .child(list),
            );
        }

        // 4. 审计记录
        let mut hist = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child(format!("审计记录（{}）", self.approval_history.len())),
            );
        if self.approval_history.is_empty() {
            hist = hist.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("暂无审批记录"),
            );
        }
        for h in self.approval_history.iter().take(20) {
            hist = hist.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(risk_color(&h.risk)))
                            .child(h.risk.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(h.description.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if h.decision.eq_ignore_ascii_case("deny") {
                                colors::ERROR()
                            } else if h.decision.eq_ignore_ascii_case("approve") {
                                colors::SUCCESS()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(h.decision.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(match h.result {
                                Some(true) => colors::SUCCESS(),
                                Some(false) => colors::ERROR(),
                                None => colors::TEXT_TERTIARY(),
                            }))
                            .child(match h.result {
                                Some(true) => "执行成功".to_string(),
                                Some(false) => "执行失败".to_string(),
                                None => "未执行".to_string(),
                            }),
                    ),
            );
        }
        body = body.child(hist);

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
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        svg()
                                            .path(IconName::Shield.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("审批"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.approvals_page_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("ap-reload")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("刷新")
                            .on_click(cx.listener(|this, _, _, cx| this.load_approval_page(cx))),
                    ),
            )
            .child(body.overflow_y_scrollbar())
    }

    pub(crate) fn load_approval_page(&self, cx: &mut Context<Self>) {
        self.load_approvals(cx);
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/status");
            match fetch_json(url).await {
                Ok(body) => {
                    let config = body.get("config");
                    let mode = config
                        .and_then(|c| c.get("mode"))
                        .map(|v| {
                            v.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string())
                        })
                        .unwrap_or_default();
                    let auto = config
                        .and_then(|c| c.get("enable_auto_approval"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let timeout = config
                        .and_then(|c| c.get("default_timeout_secs"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let max_pending = config
                        .and_then(|c| c.get("max_pending_approvals"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let prompt_cmds: Vec<String> = config
                        .and_then(|c| c.get("prompt_commands"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let rules: Vec<ApprovalRuleRow> = config
                        .and_then(|c| c.get("auto_approval_rules"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some(ApprovalRuleRow {
                                        name: r.get("name")?.as_str()?.to_string(),
                                        decision: r
                                            .get("decision")
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_default(),
                                        enabled: r
                                            .get("enabled")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(true),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let pending_conf: Vec<String> = body
                        .get("pending_confirmations")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let confirmed = body
                        .get("confirmed_action_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let history: Vec<ApprovalHistRow> = body
                        .get("recent_records")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    let req = r.get("request")?;
                                    Some(ApprovalHistRow {
                                        description: req
                                            .get("action_description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        risk: req
                                            .get("risk_level")
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_default(),
                                        decision: r
                                            .get("response")
                                            .and_then(|resp| resp.get("decision"))
                                            .map(|v| {
                                                v.as_str()
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| v.to_string())
                                            })
                                            .unwrap_or_else(|| "未决".to_string()),
                                        session_id: req
                                            .get("session_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        result: r.get("execution_result").and_then(|v| v.as_bool()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = history.len();
                    let _ = this.update(cx, |this, cx| {
                        this.approval_mode = mode;
                        this.approval_auto = auto;
                        this.approval_timeout = timeout;
                        this.approval_max_pending = max_pending;
                        this.approval_prompt_cmds = prompt_cmds;
                        this.approval_rules = rules;
                        this.pending_confirmations = pending_conf;
                        this.confirmed_action_count = confirmed;
                        this.approval_history = history;
                        this.approvals_page_status = format!("审计记录 {n} 条");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.approvals_page_status = format!("加载审批状态失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}
