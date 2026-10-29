//! 模态层：确认框、文件/目录选择器。
//!
//! 全部是 **GPUI 自绘覆盖层**（不是 OS 对话框）：OS 级模态框自动化端口既点不到也
//! 看不到（`/state` 里没有它的位置），自绘的才能被断言与驱动。
//!
//! 文件选择器一套 UI 服务两个用途（知识导入多选 / @ 引用单选），差别只在
//! `FilePickPurpose` 决定的三处：标题、点文件动作、底部按钮组。

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::Input;
use crate::types::*;

impl ClientApp {
    /// 打开「编辑后批准」弹窗（回填原命令）。
    ///
    /// 复用 `dialog_input`（与重命名同一个实体槽）：每次新建实体，因为
    /// `InputState` 没有公开的 `set_value` 可用来回填（同 `open_rename` 的理由）。
    pub(crate) fn open_edit_approval(
        &mut self,
        request_id: String,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefill = command.clone();
        self.dialog_input = Some(cx.new(|cx| {
            InputState::new(window, cx).default_value(prefill)
        }));
        self.dialog = Some(Dialog::EditApproval { request_id });
        cx.notify();
    }

    /// 「编辑后批准」提交：读输入框内容当 `edited_command`，再按 `approve` 提交。
    pub(crate) fn confirm_edit_approval(&mut self, request_id: String, cx: &mut Context<Self>) {
        let edited = self
            .dialog_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        self.dialog = None;
        self.dialog_input = None;
        self.respond_approval(request_id, "approve", Some(edited), cx);
    }

    /// 模态对话框覆盖层（删除确认 / 重命名）：整窗半透明遮罩 + 居中卡片。
    ///
    /// 自绘而不接组件库 `Dialog`：只有两种形态，自绘更可控（遮罩色、按钮危险色
    /// 都能直接对齐深色主题），也避免为弹窗引入一套焦点/Popover 语义。
    pub(crate) fn render_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.dialog.clone() else {
            return div().into_any_element();
        };
        // 目录选择器是「列表 + 逐级导航」的独立形态，不走下面那条卡片骨架
        if let Dialog::WorkspacePicker { target } = dialog {
            return self.render_workspace_picker_dialog(target, cx);
        }
        // 文件选择弹窗同理：列表 + 导航 + 选中动作，独立形态
        if let Dialog::FilePicker { purpose } = dialog {
            return self.render_file_picker_dialog(purpose, cx);
        }
        let (heading, message, confirm_label, danger) = match &dialog {
            Dialog::DeleteSession { title, .. } => (
                "删除会话".to_string(),
                Some(format!("确定删除会话「{title}」吗？此操作不可恢复。")),
                "删除".to_string(),
                true,
            ),
            Dialog::RenameSession { .. } => {
                ("重命名会话".to_string(), None, "保存".to_string(), false)
            }
            // 目录选择器是独立形态（列表 + 导航），在 `render_dialog` 开头就被分流，
            // 走不到这里；给个兜底以免新增变体时漏配
            Dialog::WorkspacePicker { .. } => {
                ("选择目录".to_string(), None, "选择".to_string(), false)
            }
            // 文件选择弹窗在 `render_dialog` 开头就分流了，走不到这里
            Dialog::FilePicker { .. } => {
                ("选择文件".to_string(), None, "确定".to_string(), false)
            }
            Dialog::ResetRole { name } => (
                "回退内置种子".to_string(),
                Some(format!(
                    "将 {name} 回退到内置种子定义（学习演化结果会被覆盖）？"
                )),
                "回退".to_string(),
                true,
            ),
            Dialog::DeleteRole { name } => (
                "退役删除角色".to_string(),
                Some(format!("退役并删除角色 {name}（定义与会话一并删除）？")),
                "删除".to_string(),
                true,
            ),
            Dialog::RollbackMessage { preview, .. } => (
                "回退到此处".to_string(),
                Some(format!(
                    "将删除「{preview}」这条消息及其之后的全部内容（含工作区文件回退），不可恢复。"
                )),
                "回退".to_string(),
                true,
            ),
            Dialog::EditApproval { .. } => (
                "编辑后批准".to_string(),
                // 后端语义必须说清：编辑值只记审计与通知，实际执行仍按原命令
                Some(
                    "改写后的命令会记入审批审计与通知，但**实际执行仍由智能体按原命令发起**。"
                        .to_string(),
                ),
                "批准".to_string(),
                false,
            ),
        };
        let mut card = div()
            .w(px(420.))
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child(heading),
            );
        if let Some(msg) = message {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(msg),
            );
        }
        // 重命名用输入框（目录选择器是独立形态，走别的分支）
        // 需要输入框的两种弹窗：重命名会话 / 审批「编辑后批准」
        if let (Dialog::RenameSession { .. } | Dialog::EditApproval { .. }, Some(input)) =
            (&dialog, self.dialog_input.as_ref())
        {
            card = card.child(
                div()
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_PRIMARY()))
                    .px_2()
                    .py_1()
                    .child(Input::new(input).appearance(false)),
            );
        }
        card = card.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    div()
                        .id("dlg-cancel")
                        .px_3()
                        .py_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("取消")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dialog = None;
                            this.dialog_input = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("dlg-confirm")
                        .px_3()
                        .py_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_sm()
                        .bg(rgb(if danger {
                            colors::ERROR()
                        } else {
                            colors::ACCENT()
                        }))
                        .text_color(rgb(0xffffff))
                        .child(confirm_label)
                        .on_click(
                            cx.listener(move |this, _, _, cx| match this.dialog.clone() {
                                Some(Dialog::DeleteSession { .. }) => this.confirm_delete(cx),
                                Some(Dialog::RenameSession { .. }) => this.submit_rename(cx),
                                Some(Dialog::WorkspacePicker { .. }) => {}
                                // 文件选择弹窗同样是独立形态（不走这条骨架）
                                Some(Dialog::FilePicker { .. }) => {}
                                Some(Dialog::ResetRole { name }) => {
                                    this.confirm_reset_role(name, cx)
                                }
                                Some(Dialog::DeleteRole { name }) => {
                                    this.confirm_delete_role(name, cx)
                                }
                                Some(Dialog::RollbackMessage {
                                    session_id,
                                    message_id,
                                    ..
                                }) => this.confirm_rollback(session_id, message_id, cx),
                                Some(Dialog::EditApproval { request_id, .. }) => {
                                    this.confirm_edit_approval(request_id, cx)
                                }
                                None => {}
                            }),
                        ),
                ),
        );
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(card)
            .into_any_element()
    }

    /// 角色操作栏：回退内置种子 / 退役删除（都走确认弹窗）。
    ///
    /// 「回退」在 `builtin && version == 1` 时禁用 —— 内置种子本来就是 v1，
    /// 没有可回退的内容（Web 版同口径）。
    /// 目录选择器弹窗：当前路径 + 逐级浏览 + 「选择此目录」。
    ///
    /// 浏览根（盘符列表）不允许直接选中 —— 后端要求的是具体目录。
    /// 文件选择弹窗（**GPUI 自绘**，不是 OS 对话框）：点目录进入、点文件勾选。
    ///
    /// 形态与 workspace 目录选择器一致（遮罩 + 居中卡片 + 列表），因为它们是同一类
    /// 任务（在文件系统里定位）。弹窗而不是平铺：导入 tab 的主任务只有「选文件 +
    /// 导入」，把一张常驻的文件列表铺在页面上会把主次关系拉平。
    /// 文件选择弹窗（知识导入 / @ 引用共用）。
    ///
    /// 两种用途的差别全部集中在本函数的三处 `purpose` 分支上：标题、点文件的
    /// 动作、底部按钮组。其余（上级目录 / 过滤 / 隐藏项 / 大小）逐字共用。
    pub(crate) fn render_file_picker_dialog(
        &self,
        purpose: FilePickPurpose,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // 列表：上级目录 → 子目录 → 文件
        let mut list = div().flex().flex_col().gap_1().w_full();
        if !self.kb_browse_dir.is_empty() && self.kb_browse_dir != "浏览根" {
            let parent = std::path::Path::new(&self.kb_browse_dir)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            list = list.child(
                div()
                    .id("kbp-up")
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
                        this.load_kb_browse(parent.clone(), cx);
                    })),
            );
        }
        if self.kb_browse_entries.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(if self.kb_browse_status.is_empty() {
                        "（空目录）"
                    } else {
                        "（空目录或读取中）"
                    }),
            );
        }
        // 遇到第一个文件时插一个「文件」分隔标题：目录总在前，没有分隔时
        // 用户看不出「下面还有文件」（截图里第一屏全是目录就是这个感受）。
        let q = self.kb_pick_query.trim().to_lowercase();
        let mut files_label_drawn = false;
        for (ix, e) in self.kb_browse_entries.iter().enumerate() {
            // 默认隐藏点开头的条目：项目根目录动辄十几个 `.git`/`.cargo`/`.vscode`，
            // 第一屏全被它们占满，真正要选的文件得滚好几屏（截图实测过）。
            if !self.kb_show_hidden && e.name.starts_with('.') {
                continue;
            }
            // 过滤词：只在「按名字」上匹配（选文件的场景里，路径信息没用）
            if !q.is_empty() && !e.name.to_lowercase().contains(&q) {
                continue;
            }
            if !e.is_dir && !files_label_drawn {
                files_label_drawn = true;
                list = list.child(
                    div()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("文件"),
                );
            }
            let is_picked = self.kb_picked.iter().any(|p| p == &e.path);
            let dir = e.path.clone();
            let is_dir = e.is_dir;
            let picked_path = e.path.clone();
            let icon = if is_dir {
                IconName::Folder
            } else {
                IconName::File
            };
            let size_text = e.size.map(format_size).unwrap_or_default();
            list =
                list.child(
                    div()
                        .id(("kbp-item", ix))
                        .w_full()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(if is_picked {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_SECONDARY()
                        }))
                        .cursor_pointer()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(svg().path(icon.path()).size(px(12.)).text_color(rgb(
                            if is_picked {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_TERTIARY()
                            },
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .text_size(px(12.))
                                .text_color(rgb(if is_picked {
                                    colors::ACCENT()
                                } else {
                                    colors::TEXT_SECONDARY()
                                }))
                                .child(e.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(size_text),
                        )
                        .child(
                            // 目录不给勾选位（点它就是进入），文件给一个明确的勾选标记
                            div()
                                .w(px(16.))
                                .flex()
                                .justify_center()
                                .text_size(px(11.))
                                .text_color(rgb(if is_picked {
                                    colors::SUCCESS()
                                } else {
                                    colors::TEXT_TERTIARY()
                                }))
                                .child(if is_dir {
                                    ""
                                } else if is_picked {
                                    "✓"
                                } else {
                                    "○"
                                }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if is_dir {
                                this.load_kb_browse(dir.clone(), cx);
                            } else if purpose == FilePickPurpose::Mention {
                                // @ 引用单选：点中即插入并关闭，不要「勾选 + 确认」两步
                                this.mention_file(picked_path.clone(), window, cx);
                            } else {
                                this.toggle_kb_pick(picked_path.clone());
                                cx.notify();
                            }
                        })),
                );
        }

        let card =
            div()
                .w(px(680.))
                .max_h(px(620.))
                .rounded_xl()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .flex()
                .flex_col()
                // 标题行
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            svg()
                                .path(IconName::FolderOpen.path())
                                .size(px(16.))
                                .text_color(rgb(colors::TEXT_SECONDARY())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(14.))
                                .text_color(rgb(colors::TEXT_PRIMARY()))
                                .child(match purpose {
                                    FilePickPurpose::KbIngest => "选择要导入知识库的文件",
                                    FilePickPurpose::Mention => "选择要引用的文件（点中即插入）",
                                }),
                        )
                        .child(
                            div()
                                .id("kbp-close")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child("✕")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                })),
                        ),
                )
                // 路径行
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(colors::BORDER()))
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
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(if self.kb_browse_dir.is_empty() {
                                    "浏览根".to_string()
                                } else {
                                    self.kb_browse_dir.clone()
                                }),
                        )
                        .child(
                            div()
                                .id("kbp-root")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::ACCENT()))
                                .child("回到根")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.load_kb_browse(String::new(), cx)
                                })),
                        ),
                )
                // 隐藏项开关（默认隐藏，见列表循环里的过滤）
                .child(
                    div()
                        .px_4()
                        .pb_2()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors::BORDER()))
                                .bg(rgb(colors::BG_PRIMARY()))
                                .px_2()
                                .py_1()
                                .child(Input::new(&self.kb_pick_search).appearance(false)),
                        )
                        .child(
                            div()
                                .id("kbp-hidden")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if self.kb_show_hidden {
                                    colors::ACCENT()
                                } else {
                                    colors::BORDER()
                                }))
                                .cursor_pointer()
                                .text_size(px(10.))
                                .text_color(rgb(if self.kb_show_hidden {
                                    colors::ACCENT()
                                } else {
                                    colors::TEXT_TERTIARY()
                                }))
                                .child(if self.kb_show_hidden {
                                    "隐藏项：显示"
                                } else {
                                    "隐藏项：隐藏"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.kb_show_hidden = !this.kb_show_hidden;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(if self.kb_browse_status.is_empty() {
                                    String::new()
                                } else {
                                    self.kb_browse_status.clone()
                                }),
                        ),
                )
                // 列表（确定高度 + 自己滚动）
                .child(
                    div()
                        .h(px(400.))
                        .min_h_0()
                        .px_3()
                        .py_2()
                        .child(list.overflow_y_scrollbar()),
                )
                // 底部：已选 + 清空 / 取消 / 导入
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(rgb(colors::BORDER()))
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
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(match purpose {
                                    FilePickPurpose::Mention => {
                                        "点目录进入，点文件即插入引用".to_string()
                                    }
                                    FilePickPurpose::KbIngest => {
                                        if self.kb_picked.is_empty() {
                                            "点目录进入，点文件勾选".to_string()
                                        } else {
                                            format!("已选 {} 个文件", self.kb_picked.len())
                                        }
                                    }
                                }),
                        )
                        // 「清空 / 导入选中」只属于多选的导入用途；引用是单选即插入，
                        // 留着这两个按钮只会让人以为要先勾选
                        .children(if purpose == FilePickPurpose::KbIngest {
                            Some(
                                div()
                                    .id("kbp-clear")
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("清空")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.kb_picked.clear();
                                        cx.notify();
                                    })),
                            )
                        } else {
                            None
                        })
                        .child(
                            div()
                                .id("kbp-cancel")
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(colors::BORDER()))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child("取消")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.notify();
                                })),
                        )
                        .children(if purpose == FilePickPurpose::KbIngest {
                            Some(
                                div()
                                    .id("kbp-confirm")
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT()))
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child("导入选中")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.import_kb_picked(cx);
                                        this.dialog = None;
                                        cx.notify();
                                    })),
                            )
                        } else {
                            None
                        }),
                );

        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(card)
            .into_any_element()
    }

    pub(crate) fn render_workspace_picker_dialog(
        &self,
        target: PickerTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_col().gap_1().w_full();
        if let Some(parent) = self.picker_parent.clone() {
            list = list.child(
                div()
                    .id("picker-up")
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
                        this.load_picker_dirs(parent.clone(), cx);
                    })),
            );
        }
        for (ix, (name, path)) in self.picker_entries.iter().enumerate() {
            let path = path.clone();
            list = list.child(
                div()
                    .id(("picker-dir", ix))
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
                            .path(IconName::Folder.path())
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
                            .child(name.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_picker_dirs(path.clone(), cx);
                    })),
            );
        }
        if self.picker_entries.is_empty() && self.picker_parent.is_none() {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(self.picker_status.clone()),
            );
        }

        let can_pick = !self.picker_current.is_empty() && self.picker_current != "浏览根";
        let pick_label = match target {
            PickerTarget::NewSession => "用它新建会话",
            PickerTarget::Session => "绑定到当前会话",
        };

        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a0))
            .child(
                div()
                    .w(px(560.))
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(15.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("选择工作目录"),
                    )
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
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(self.picker_current.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.picker_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            // 必须给**确定高度**：`overflow_y_scrollbar()` 内部按父高布局，
                            // 父容器只有 `max_h` 时高度会塌成 0（实测列表区只留一条空白）。
                            .h(px(300.))
                            .min_h_0()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_PRIMARY()))
                            .p_2()
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("picker-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dialog = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("picker-confirm")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .text_sm()
                                    .bg(rgb(if can_pick {
                                        colors::ACCENT()
                                    } else {
                                        colors::BG_TERTIARY()
                                    }))
                                    .text_color(rgb(if can_pick {
                                        0xffffff
                                    } else {
                                        colors::TEXT_TERTIARY()
                                    }))
                                    .child(pick_label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_pick {
                                            this.confirm_workspace_picker(target, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 打开目录选择器（先拉浏览根 = 盘符/家目录，再逐级进入）。
    ///
    /// 取代此前的「手输绝对路径」：路径打错不会有任何提示，浏览选择才是正常交互。
    pub(crate) fn open_workspace_picker(&mut self, target: PickerTarget, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::WorkspacePicker { target });
        self.picker_current.clear();
        self.picker_parent = None;
        self.picker_entries.clear();
        self.picker_status = "正在读取目录…".to_string();
        self.load_picker_dirs(String::new(), cx);
        cx.notify();
    }

    /// 拉某一级目录的子目录列表（`GET /workspace/dirs?path=`；空串 = 浏览根）。
    pub(crate) fn load_picker_dirs(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = if path.is_empty() {
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/dirs")
            } else {
                format!(
                    "http://{SERVER_HOST}:{port}/api/v1/workspace/dirs?path={}",
                    url_encode(&path)
                )
            };
            match fetch_json(url).await {
                Ok(body) => {
                    let current = body
                        .get("current")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let parent = body
                        .get("parent")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let entries: Vec<(String, String)> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some((
                                        e.get("name")?.as_str()?.to_string(),
                                        e.get("path")?.as_str()?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = entries.len();
                    let _ = this.update(cx, |this, cx| {
                        this.picker_current = current;
                        this.picker_parent = parent;
                        this.picker_entries = entries;
                        this.picker_status = format!("{n} 个子目录");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.picker_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 目录选择器「选这个目录」：按目标分派（新会话待绑定 / 绑定当前会话）。
    pub(crate) fn confirm_workspace_picker(&mut self, target: PickerTarget, cx: &mut Context<Self>) {
        // 浏览根（未进入任何盘符）时不可选
        if self.picker_current.is_empty() || self.picker_current == "浏览根" {
            self.picker_status = "请先进入一个目录".to_string();
            cx.notify();
            return;
        }
        let path = self.picker_current.clone();
        self.dialog = None;
        match target {
            PickerTarget::NewSession => {
                self.new_session_workspace = Some(path.clone());
                self.start_new_session(cx);
                self.status = format!("新会话将绑定目录：{path}");
                cx.notify();
            }
            PickerTarget::Session => {
                let Some(sid) = self.current_session.clone() else {
                    self.ws_status = "没有活动会话，无法绑定目录".to_string();
                    cx.notify();
                    return;
                };
                let port = self.port;
                cx.spawn(async move |this, cx| {
                    let url =
                        format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/workspace");
                    match put_json(url, serde_json::json!({ "working_directory": path })).await {
                        Ok(_) => {
                            let _ = this.update(cx, |this, cx| {
                                if let Some(s) = this.sessions_data.iter_mut().find(|s| s.id == sid)
                                {
                                    s.working_directory = path.clone();
                                }
                                // 目录变了：文件页缓存作废并重新拉树
                                this.ws_root.clear();
                                this.ws_entries.clear();
                                this.ws_file = None;
                                this.ws_path.clear();
                                this.load_workspace_tree(String::new(), cx);
                                this.rebuild_session_rows(cx);
                                cx.notify();
                            });
                        }
                        Err(e) => {
                            let _ = this.update(cx, |this, cx| {
                                this.ws_status = format!("绑定目录失败：{e}");
                                cx.notify();
                            });
                        }
                    }
                })
                .detach();
            }
        }
    }

    /// 加载子智能体列表（`GET /roles`）。
    /// 加载审批页（`GET /approval/status`：工作流配置 + 待处理 + 审计）。
    ///
    /// 同时刷一次待审批列表（`load_approvals`）—— 那个走的是同一份
    /// `pending_approvals`，两个入口（对话横幅 / 审批页）必须看到同一批数据。
    /// 打开文件选择弹窗：默认定位到「当前会话的工作目录」→「用户主目录」。
    ///
    /// 直接落在盘符根是很糟的默认值 —— 一屏全是 `hiberfil.sys` / `pagefile.sys`
    /// 这类系统文件，既找不到东西也说明不了从哪开始找。
    pub(crate) fn open_kb_picker(&mut self, cx: &mut Context<Self>) {
        self.open_file_picker(FilePickPurpose::KbIngest, cx);
    }

    /// 打开文件选择弹窗（知识导入 / @ 引用共用一套浏览 UI）。
    pub(crate) fn open_file_picker(&mut self, purpose: FilePickPurpose, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::FilePicker { purpose });
        self.kb_picked.clear();
        let start = self.kb_picker_start_dir();
        self.load_kb_browse(start, cx);
        cx.notify();
    }

    /// 文件选择弹窗的起始目录。
    pub(crate) fn kb_picker_start_dir(&self) -> String {
        if let Some(sid) = &self.current_session {
            if let Some(s) = self.sessions_data.iter().find(|s| &s.id == sid) {
                if !s.working_directory.is_empty() {
                    return s.working_directory.clone();
                }
            }
        }
        std::env::var("USERPROFILE").unwrap_or_default()
    }

    /// 文件选择弹窗的数据源：列某一级（目录 + 文件，`include_files=true`）。
    ///
    /// 走 `/workspace/dirs` 而不是新接口：同一个目录遍历，只是多一个开关
    /// （目录选择器仍只要目录）。空路径 = 浏览根（盘符列表）。
    pub(crate) fn load_kb_browse(&self, path: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let mut url =
                format!("http://{SERVER_HOST}:{port}/api/v1/workspace/dirs?include_files=true");
            if !path.is_empty() {
                url.push_str(&format!("&path={}", url_encode(&path)));
            }
            match fetch_json(url).await {
                Ok(body) => {
                    let current = body
                        .get("current")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let rows: Vec<KbBrowseEntry> = body
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    Some(KbBrowseEntry {
                                        name: e.get("name")?.as_str()?.to_string(),
                                        path: e.get("path")?.as_str()?.to_string(),
                                        is_dir: e
                                            .get("is_dir")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false),
                                        size: e.get("size").and_then(|v| v.as_u64()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.kb_browse_dir = current;
                        this.kb_browse_entries = rows;
                        this.kb_browse_status = format!("{n} 项");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.kb_browse_status = format!("读取目录失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 勾选 / 取消勾选一个文件（目录不参与勾选，点它只是进入）。
    pub(crate) fn toggle_kb_pick(&mut self, path: String) {
        if let Some(pos) = self.kb_picked.iter().position(|p| p == &path) {
            self.kb_picked.remove(pos);
        } else {
            self.kb_picked.push(path);
        }
    }

    /// 导入已勾选的文件（浏览器路径与「按路径导入」共用 [`ClientApp::kb_ingest_paths`]）。
    pub(crate) fn import_kb_picked(&mut self, cx: &mut Context<Self>) {
        if self.kb_picked.is_empty() {
            self.kb_status = "还没勾选文件".to_string();
            cx.notify();
            return;
        }
        let paths: Vec<std::path::PathBuf> = self
            .kb_picked
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        let category = self.kb_ingest_category.read(cx).value().trim().to_string();
        self.kb_ingest_paths(paths, category, cx);
    }

    /// 回退内置种子（`POST /roles/{name}/reset`）：成功后刷新列表 / 统计 / 详情。
    ///
    /// 服务端语义是「下次会话边界生效」——已在跑的会话仍用旧定义，所以提示里
    /// 明说这一点，避免用户以为立刻变了。
    pub(crate) fn confirm_reset_role(&mut self, name: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}/reset");
            match post_json(url, serde_json::json!({})).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg =
                            Some("已回退内置种子（下次会话边界生效）".to_string());
                        this.load_roles(cx);
                        this.load_roles_stats(cx);
                        this.load_role_detail(name.clone(), cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some(format!("操作失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 退役删除角色（`DELETE /roles/{name}`）：成功后清空选中并刷新列表。
    pub(crate) fn confirm_delete_role(&mut self, name: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}");
            match delete_json(url).await {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some("角色已退役删除".to_string());
                        if this.roles_selected.as_deref() == Some(name.as_str()) {
                            this.roles_selected = None;
                            this.role_detail = None;
                        }
                        this.load_roles(cx);
                        this.load_roles_stats(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_action_msg = Some(format!("操作失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}
