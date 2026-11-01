#![allow(dead_code)]

//! 输入框的 GPUI 层（第二阶段：自绘渲染）。
//!
//! 分工：[`crate::composer`] 是**纯逻辑**（文本 / 光标 / 选区 / 原子区间，可单测）；
//! 本模块把它排成「视觉行」并画出来 —— 文本段走 GPUI 的 shaping（`shape_line`），
//! 文件引用画成 chip（圆角矩形 + 文件名），光标与选区按字节偏移定位。
//!
//! ⚠️ 输入（键盘 / IME）与接入 `ClientApp` 在下一阶段；本阶段只保证「给定文本能正确
//! 排版并绘制」，好让渲染先可截图比对。
//!
//! 排版口径（改之前先读）：
//! - 先按 `\n` **硬分行**，行内再按文件引用切成「文本段 / 引用块」；
//! - 引用是**原子块**：宽度固定、不拆分，放不下就整体换到下一视觉行；
//! - 文本段放不下时按**字节**二分切分（`LineLayout::x_for_index` 的索引单位是字节，
//!   与 `Buffer.cursor` 同一口径），切点向下取整到字符边界；
//! - 行高统一取 `window.line_height()`（不做行内多字号，输入框不需要）。

use std::cell::{Cell, RefCell};
use std::ops::Range;

use crate::composer::{file_refs, Buffer};
use crate::theme::colors;
use crate::*;

/// 输入框最少显示 1 行。
pub(crate) const COMPOSER_MIN_ROWS: usize = 1;

/// 输入框最多显示 8 行（超过后内部滚动，对齐 vite 的 auto-grow 上限）。
pub(crate) const COMPOSER_MAX_ROWS: usize = 8;

/// chip 左右内边距（像素）。
const CHIP_PAD_X: f32 = 8.0;

/// 输入框向宿主（`ClientApp`）发出的事件。
#[derive(Clone, Debug)]
pub(crate) enum ComposerEvent {
    /// 回车（无 Shift）—— 宿主负责发送。
    Submit,
    /// 粘贴载荷里带**图片** —— 交给宿主的图片通道（附件区缩略图）。
    PasteImage(gpui_kit::ClipboardItem),
}

impl gpui_kit::EventEmitter<ComposerEvent> for ComposerState {}

/// 输入框状态：编辑核心 + 焦点 + IME 组合态 + 撤销栈。
pub(crate) struct ComposerState {
    /// 编辑核心（纯逻辑，见 [`crate::composer::Buffer`]）。
    pub(crate) buffer: Buffer,
    /// 焦点句柄（`track_focus` / IME / 光标可见性都看它）。
    pub(crate) focus_handle: FocusHandle,
    /// IME 组合态（字节区间）：组合中的文字加下划线。
    pub(crate) marked: Option<Range<usize>>,
    /// 撤销栈（文本 + 光标快照，上限 200）。
    pub(crate) undo: Vec<(String, usize)>,
    /// 垂直滚动（像素；内容超出 8 行时生效）。
    pub(crate) scroll: f32,
    /// 最近一次排版的命中表（鼠标定位用；绘制阶段刷新）。
    hits: RefCell<Vec<HitRow>>,
    /// 最近一次绘制的元素原点（鼠标坐标换算用）。
    origin: RefCell<(f32, f32)>,
    /// 最近一次粘贴的时刻（`Ctrl+V` 与平台 `paste` 可能同时到达，靠它去重）。
    last_paste: Option<std::time::Instant>,
    /// 上一帧排版出的**真实视觉行数**（下一帧 `request_layout` 拿它定高）。
    ///
    /// 为什么需要：高度必须在布局阶段定，而软换行只有排版后才知道要几行。
    /// 首帧按估算给、之后用真实值自愈 —— 少了这一步，长文本会「换行了但看不见」
    /// （多出来的行被元素高度裁掉，用户看到的就是「不能自动换行」）。
    pub(crate) rows_cache: Cell<usize>,
    /// 上一帧的可用宽度（估算行数用；首帧用窗口宽近似）。
    avail_cache: Cell<f32>,
    /// 光标是否处于「亮」相位（闪烁）。
    caret_on: Cell<bool>,
    /// 是否正在按住鼠标拖拽选区（on_mouse_down 置真、on_mouse_up 置假）。
    dragging: bool,
    /// 闪烁任务（只启动一次）。
    blink_task: Option<gpui_kit::Task<()>>,
}

impl ComposerState {
    /// 新建一个空输入框。
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            buffer: Buffer::default(),
            focus_handle: cx.focus_handle(),
            marked: None,
            undo: Vec::new(),
            scroll: 0.0,
            hits: RefCell::new(Vec::new()),
            origin: RefCell::new((0.0, 0.0)),
            last_paste: None,
            rows_cache: Cell::new(0),
            avail_cache: Cell::new(0.0),
            caret_on: Cell::new(true),
            dragging: false,
            blink_task: None,
        }
    }

    /// 当前正文（自动化 `composer_text` / 发送都用它）。
    pub(crate) fn value(&self) -> String {
        self.buffer.text.clone()
    }

    /// 整体替换正文（草稿恢复 / 自动化写入），光标落到末尾。
    pub(crate) fn set_value(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.buffer.set_text(text);
        self.marked = None;
        self.scroll = 0.0;
        cx.notify();
    }

    /// 清空。
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_value("", cx);
    }

    /// 请求聚焦（自动化 `composer_focus` 与鼠标点击共用）。
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    /// 在光标处插入一个文件引用（粘贴文件的落点）。
    pub(crate) fn insert_file_ref(&mut self, path: &str, cx: &mut Context<Self>) {
        self.push_undo();
        self.buffer.insert_file_ref(path);
        cx.notify();
    }

    /// 推一条撤销快照（同一文本连续操作不重复入栈）。
    pub(crate) fn push_undo(&mut self) {
        if self.undo.last().map(|(t, _)| t.as_str()) == Some(self.buffer.text.as_str()) {
            return;
        }
        self.undo
            .push((self.buffer.text.clone(), self.buffer.cursor));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
    }

    /// 撤销一步。
    pub(crate) fn undo_once(&mut self, cx: &mut Context<Self>) {
        if let Some((text, cursor)) = self.undo.pop() {
            self.buffer.set_text(text);
            self.buffer.cursor = cursor.min(self.buffer.text.len());
            self.marked = None;
            cx.notify();
        }
    }

    /// 估算可见行数（首帧用；之后由 [`Self::rows_cache`] 的真实值接管）。
    ///
    /// `avail` = 上一帧的可用宽度（≤ 0 时退化为「窗口宽 − 400」的粗估）。
    pub(crate) fn estimated_rows(&self, window: &mut Window, avail: f32) -> usize {
        let font_size = f32::from(window.text_style().font_size.to_pixels(window.rem_size()));
        let per_char = (font_size * 0.6).max(6.0);
        let avail = if avail > 32.0 {
            avail
        } else {
            (f32::from(window.viewport_size().width) - 400.0).max(160.0)
        };
        let mut rows = 0usize;
        for line in self.buffer.text.split('\n') {
            let w = line.chars().count() as f32 * per_char;
            rows += ((w / avail.max(per_char)).ceil() as usize).max(1);
        }
        rows.clamp(COMPOSER_MIN_ROWS, COMPOSER_MAX_ROWS)
    }

    /// 启动光标闪烁（530ms 一个相位；只在第一次渲染时起任务）。
    fn start_caret_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blink_task.is_some() {
            return;
        }
        self.blink_task = Some(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(530))
                .await;
            let alive = this
                .update(cx, |this, cx| {
                    this.caret_on.set(!this.caret_on.get());
                    cx.notify();
                })
                .is_ok();
            if !alive {
                break;
            }
        }));
    }

    /// 命中测试：**元素内坐标** → 正文字节位置（调用方先减去元素原点）。
    pub(crate) fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        let hits = self.hits.borrow();
        let y = y + self.scroll;
        let row = hits.iter().find(|r| y >= r.top && y < r.top + r.height)?;
        Some(row.byte_at(x))
    }
}

/// 命中表里的一行。
struct HitRow {
    top: f32,
    height: f32,
    pieces: Vec<HitPiece>,
}

impl HitRow {
    /// `x` → 字节位置（落在 chip 上取块首/块尾，落在文本上走 shaping 的反查）。
    fn byte_at(&self, x: f32) -> usize {
        for p in &self.pieces {
            if x >= p.x && x < p.x + p.width {
                return match &p.kind {
                    HitKind::Chip => {
                        if x < p.x + p.width / 2.0 {
                            p.byte.start
                        } else {
                            p.byte.end
                        }
                    }
                    HitKind::Text(line) => {
                        let local = line.index_for_x(px(x - p.x)).unwrap_or(0);
                        p.byte.start + local
                    }
                };
            }
        }
        // 落在行尾之后 → 行末
        self.pieces.last().map(|p| p.byte.end).unwrap_or(0)
    }
}

/// 命中表里的一个片段。
struct HitPiece {
    x: f32,
    width: f32,
    byte: Range<usize>,
    kind: HitKind,
}

/// 片段类型（文本带 shaping 结果，chip 只有范围）。
enum HitKind {
    Text(Box<ShapedLine>),
    Chip,
}

/// 排版结果：视觉行（绘制与命中都用它）。
pub(crate) struct Prelayout {
    rows: Vec<VRow>,
    width: Pixels,
    line_height: Pixels,
}

struct VRow {
    top: f32,
    pieces: Vec<Piece>,
}

struct Piece {
    x: f32,
    width: f32,
    byte: Range<usize>,
    kind: PieceKind,
}

enum PieceKind {
    Text(Box<ShapedLine>),
    Chip { shaped: Box<ShapedLine> },
}

impl Element for ComposerBody {
    type RequestLayoutState = ();
    type PrepaintState = Prelayout;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("composer-body".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 高度：优先用上一帧排版出的**真实行数**，首帧才估算（见 `rows_cache`）
        let rows = {
            let st = self.state.read(cx);
            let cached = st.rows_cache.get();
            if cached > 0 {
                cached.min(COMPOSER_MAX_ROWS)
            } else {
                st.estimated_rows(window, st.avail_cache.get())
            }
        };
        let line_height = window.line_height();
        let style = Style {
            size: size(relative(1.0).into(), (line_height * rows as f32).into()),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text = self.state.read(cx).buffer.text.clone();
        let pre = layout_rows(&text, bounds.size.width, window, cx);
        // 命中表按「相对内容顶」记录，滚动偏移在命中时再加
        {
            let st = self.state.read(cx);
            st.hits.replace(pre.hit_rows());
            st.origin
                .replace((f32::from(bounds.origin.x), f32::from(bounds.origin.y)));
            // 真实行数 / 宽度回写：下一帧 `request_layout` 据此定高（自愈 ——
            // 少了这一步，长文本会「换行了但被元素高度裁掉」，看起来像不换行）
            st.rows_cache.set(pre.rows.len().max(COMPOSER_MIN_ROWS));
            st.avail_cache.set(f32::from(bounds.size.width));
        }
        pre
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (cursor, anchor, scroll, focused, marked) = {
            let st = self.state.read(cx);
            (
                st.buffer.cursor,
                st.buffer.anchor,
                st.scroll,
                st.focus_handle.is_focused(window),
                st.marked.clone(),
            )
        };
        let line_height = prepaint.line_height;
        let selection = match anchor {
            Some(a) if a != cursor => a.min(cursor)..a.max(cursor),
            _ => cursor..cursor,
        };
        // IME / 字符输入：把本实体注册为输入处理器（只能在 paint 阶段调用）
        window.handle_input(
            &self.state.read(cx).focus_handle.clone(),
            gpui_kit::ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );
        let accent = Hsla::from(rgb(colors::ACCENT()));
        let sel_bg = Hsla {
            a: 0.22,
            ..Hsla::from(rgb(colors::ACCENT()))
        };

        // 滚动：内容整体上移，并限制在本元素范围内
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for row in &prepaint.rows {
                let y = bounds.origin.y + px(row.top) - px(scroll);
                if y + line_height < bounds.origin.y || y > bounds.origin.y + bounds.size.height {
                    continue;
                }
                for piece in &row.pieces {
                    let x = bounds.origin.x + px(piece.x);
                    match &piece.kind {
                        PieceKind::Text(shaped) => {
                            // 选区背景（先画，文本压在上面）
                            if selection.start < piece.byte.end && piece.byte.start < selection.end
                            {
                                let from = selection.start.max(piece.byte.start);
                                let to = selection.end.min(piece.byte.end);
                                let x0 = x + shaped.x_for_index(from - piece.byte.start);
                                let x1 = x + shaped.x_for_index(to - piece.byte.start);
                                paint_quad(
                                    window,
                                    x0,
                                    y,
                                    (x1 - x0).max(px(1.)),
                                    line_height,
                                    sel_bg,
                                );
                            }
                            let _ = shaped.paint(
                                gpui_kit::point(x, y),
                                line_height,
                                gpui_kit::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                        PieceKind::Chip { shaped } => {
                            let chip_h = line_height;
                            let quad_bounds = Bounds::new(
                                gpui_kit::point(x, y + px(1.)),
                                gpui_kit::size(px(piece.width), px(f32::from(chip_h) - 2.)),
                            );
                            // 选区覆盖整个 chip（原子块被选中时整块高亮）
                            let bg = if selection.start < piece.byte.end
                                && piece.byte.start < selection.end
                            {
                                sel_bg
                            } else {
                                Hsla::from(rgb(colors::BG_TERTIARY()))
                            };
                            window.paint_quad(gpui_kit::PaintQuad {
                                bounds: quad_bounds,
                                corner_radii: gpui_kit::Corners::all(px(6.)),
                                background: bg.into(),
                                border_widths: gpui_kit::Edges::all(px(1.)),
                                border_color: Hsla::from(rgb(colors::BORDER())),
                                border_style: gpui_kit::BorderStyle::Solid,
                            });
                            let _ = shaped.paint(
                                gpui_kit::point(x + px(CHIP_PAD_X), y),
                                line_height,
                                gpui_kit::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    }
                }
                // 选区跨行的空行也要有可见宽度：交给上面的文本段处理，这里不额外画
            }

            // 光标：聚焦时按 530ms 相位闪烁（用户实测指出「光标不闪，像卡住了」）
            if focused && self.state.read(cx).caret_on.get() {
                let (cx_pos, cy) = caret_position(prepaint, cursor, bounds, scroll);
                // 光标高度：line_height 含 ascent+descent+行距，按整行画会显得偏长
                // （用户实测「稍微长了那么一点点」）→ 取字形高度的 0.85 并垂直居中
                let caret_h = line_height * 0.85;
                let dy = (line_height - caret_h) * 0.5;
                paint_quad(window, cx_pos, cy + dy, px(2.), caret_h, accent);
            }
            // IME 组合态下划线（组合中的文字位置）
            if let Some(m) = marked {
                let (x0, y0) = caret_position(prepaint, m.start, bounds, scroll);
                let (x1, _) = caret_position(prepaint, m.end, bounds, scroll);
                paint_quad(
                    window,
                    x0,
                    y0 + line_height - px(2.),
                    (x1 - x0).max(px(1.)),
                    px(2.),
                    accent,
                );
            }
        });
    }
}

/// 自绘元素：把正文排成视觉行并画出来。
pub(crate) struct ComposerBody {
    pub(crate) state: Entity<ComposerState>,
}

impl IntoElement for ComposerBody {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Render for ComposerState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 光标闪烁：第一次渲染时起一个 530ms 的相位任务
        self.start_caret_blink(window, cx);
        div()
            .id("composer-root")
            .w_full()
            .track_focus(&self.focus_handle)
            .cursor_text()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(Self::on_mouse_down),
            )
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(gpui_kit::MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(ComposerBody { state: cx.entity() })
    }
}

impl ComposerState {
    /// 滚轮：内容超出一屏时滚动（滚到边界就不动）。
    fn on_scroll(
        &mut self,
        ev: &gpui_kit::ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dy = f32::from(ev.delta.pixel_delta(window.line_height()).y);
        let content = self
            .hits
            .borrow()
            .last()
            .map(|r| r.top + r.height)
            .unwrap_or(0.0);
        let visible = f32::from(window.line_height()) * COMPOSER_MAX_ROWS as f32;
        let max = (content - visible).max(0.0);
        self.scroll = (self.scroll - dy).clamp(0.0, max);
        cx.notify();
    }
}

// ── 排版 ──────────────────────────────────────────────────────────────

/// 把正文排成视觉行。
///
/// `avail_w` 是可用宽度（元素宽）。文本段超宽时按字节二分切分（见文件头的口径）。
fn layout_rows(text: &str, avail_w: Pixels, window: &mut Window, _cx: &mut App) -> Prelayout {
    let font_size = window.text_style().font_size.to_pixels(window.rem_size());
    let font = window.text_style().font();
    let color = window.text_style().color;
    let line_height = window.line_height();
    let avail = f32::from(avail_w).max(32.0);

    let mut rows: Vec<VRow> = Vec::new();
    let mut cur = VRow {
        top: 0.0,
        pieces: Vec::new(),
    };
    let mut x = 0.0f32;
    let mut line_start = 0usize;

    for hard in text.split('\n') {
        let hard_end = line_start + hard.len();
        if hard.is_empty() {
            // 空行也要占一行高度
            rows.push(std::mem::replace(
                &mut cur,
                VRow {
                    top: 0.0,
                    pieces: Vec::new(),
                },
            ));
            x = 0.0;
            line_start = hard_end + 1;
            continue;
        }
        let mut seg_start = 0usize;
        for r in file_refs(hard) {
            if seg_start < r.range.start {
                let seg = &hard[seg_start..r.range.start];
                push_text_segment(
                    &mut rows,
                    &mut cur,
                    &mut x,
                    seg,
                    line_start + seg_start,
                    avail,
                    font_size,
                    &font,
                    color,
                    line_height,
                    window,
                );
            }
            // chip 原子块：宽度用**真实 shaping 宽度**（估算会偏窄，文字会溢出色块）
            let name = file_name_of(&r.path);
            let label = format!(" {name} ");
            let shaped = shape_text(&label, font_size, font.clone(), color, window);
            let chip_w = f32::from(shaped.width()) + CHIP_PAD_X * 2.0;
            if x > 0.0 && x + chip_w > avail {
                rows.push(std::mem::replace(
                    &mut cur,
                    VRow {
                        top: 0.0,
                        pieces: Vec::new(),
                    },
                ));
                x = 0.0;
            }
            cur.pieces.push(Piece {
                x,
                width: chip_w,
                byte: (line_start + r.range.start)..(line_start + r.range.end),
                kind: PieceKind::Chip {
                    shaped: Box::new(shaped),
                },
            });
            x += chip_w + 2.0;
            seg_start = r.range.end;
        }
        if seg_start < hard.len() {
            let seg = &hard[seg_start..];
            push_text_segment(
                &mut rows,
                &mut cur,
                &mut x,
                seg,
                line_start + seg_start,
                avail,
                font_size,
                &font,
                color,
                line_height,
                window,
            );
        }
        rows.push(std::mem::replace(
            &mut cur,
            VRow {
                top: 0.0,
                pieces: Vec::new(),
            },
        ));
        x = 0.0;
        line_start = hard_end + 1;
    }
    if !cur.pieces.is_empty() {
        rows.push(cur);
    }

    // 行 top 依次累加
    let mut top = 0.0f32;
    for row in &mut rows {
        row.top = top;
        top += f32::from(line_height);
    }

    Prelayout {
        rows,
        width: avail_w,
        line_height,
    }
}

impl Prelayout {
    /// 抽出命中表（光标定位用）。
    fn hit_rows(&self) -> Vec<HitRow> {
        self.rows
            .iter()
            .map(|r| HitRow {
                top: r.top,
                height: f32::from(self.line_height),
                pieces: r
                    .pieces
                    .iter()
                    .map(|p| HitPiece {
                        x: p.x,
                        width: p.width,
                        byte: p.byte.clone(),
                        kind: match &p.kind {
                            PieceKind::Text(line) => HitKind::Text(Box::new((**line).clone())),
                            PieceKind::Chip { .. } => HitKind::Chip,
                        },
                    })
                    .collect(),
            })
            .collect()
    }
}

/// 把一段纯文本放进视觉行（超宽时按字节二分切分）。
#[allow(clippy::too_many_arguments)]
fn push_text_segment(
    rows: &mut Vec<VRow>,
    cur: &mut VRow,
    x: &mut f32,
    seg: &str,
    base_byte: usize,
    avail: f32,
    font_size: Pixels,
    font: &gpui_kit::Font,
    color: Hsla,
    line_height: Pixels,
    window: &mut Window,
) {
    let mut rest_start = 0usize;
    while rest_start < seg.len() {
        let rest = &seg[rest_start..];
        let shaped = shape_text(rest, font_size, font.clone(), color, window);
        let w = f32::from(shaped.width());
        let remaining = avail - *x;
        if w <= remaining || *x == 0.0 {
            if w > remaining && *x == 0.0 {
                // 整行都放不下 → 硬切
                let cut = split_index(&shaped, rest, remaining.max(1.0));
                if cut == 0 {
                    // 一个字符都放不下：放一个字符（防止死循环）
                    let one = next_boundary(rest, 0);
                    cur.pieces.push(Piece {
                        x: *x,
                        width: w,
                        byte: base_byte + rest_start..base_byte + rest_start + one,
                        kind: PieceKind::Text(Box::new(shaped)),
                    });
                    *x += w;
                    rest_start += one;
                    continue;
                }
                let head = shape_text(&rest[..cut], font_size, font.clone(), color, window);
                let head_w = f32::from(head.width());
                cur.pieces.push(Piece {
                    x: *x,
                    width: head_w,
                    byte: base_byte + rest_start..base_byte + rest_start + cut,
                    kind: PieceKind::Text(Box::new(head)),
                });
                rows.push(std::mem::replace(
                    cur,
                    VRow {
                        top: 0.0,
                        pieces: Vec::new(),
                    },
                ));
                *x = 0.0;
                rest_start += cut;
                continue;
            }
            cur.pieces.push(Piece {
                x: *x,
                width: w,
                byte: base_byte + rest_start..base_byte + seg.len(),
                kind: PieceKind::Text(Box::new(shaped)),
            });
            *x += w;
            break;
        }
        // 放不下：切一段放进当前行尾，其余换行
        let cut = split_index(&shaped, rest, remaining);
        if cut == 0 {
            rows.push(std::mem::replace(
                cur,
                VRow {
                    top: 0.0,
                    pieces: Vec::new(),
                },
            ));
            *x = 0.0;
            continue;
        }
        let head = shape_text(&rest[..cut], font_size, font.clone(), color, window);
        let head_w = f32::from(head.width());
        cur.pieces.push(Piece {
            x: *x,
            width: head_w,
            byte: base_byte + rest_start..base_byte + rest_start + cut,
            kind: PieceKind::Text(Box::new(head)),
        });
        rows.push(std::mem::replace(
            cur,
            VRow {
                top: 0.0,
                pieces: Vec::new(),
            },
        ));
        *x = 0.0;
        rest_start += cut;
    }
    let _ = line_height;
}

/// 在 `rest` 里找「宽度不超过 `max_w` 的最大字节切点」（向下取整到字符边界）。
fn split_index(shaped: &ShapedLine, rest: &str, max_w: f32) -> usize {
    // 二分：x_for_index 单调不减
    let mut lo = 0usize;
    let mut hi = rest.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if f32::from(shaped.x_for_index(mid)) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut cut = lo.min(rest.len());
    while cut > 0 && !rest.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

/// 下一个字符边界（至少前进 1 字节）。
fn next_boundary(s: &str, from: usize) -> usize {
    let mut i = (from + 1).min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// shaping 一段文本（单 run）。
fn shape_text(
    text: &str,
    font_size: Pixels,
    font: gpui_kit::Font,
    color: Hsla,
    window: &mut Window,
) -> ShapedLine {
    window.text_system().shape_line(
        text.to_string().into(),
        font_size,
        &[gpui_kit::TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    )
}

/// chip 宽度：文件名按半角单位折算（中文 2、其余 1）+ 两侧内边距。
fn chip_width(name: &str, font_size: Pixels) -> f32 {
    let units: f32 = name
        .chars()
        .map(|c| if (c as u32) > 0x2E80 { 2.0 } else { 1.0 })
        .sum();
    (units * f32::from(font_size) * 0.58 + CHIP_PAD_X * 2.0).max(56.0)
}

/// 取文件名（chip 上只显示名字，全路径留在正文里）。
fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// 光标（或任意字节位）的屏幕坐标。
fn caret_position(
    pre: &Prelayout,
    byte: usize,
    bounds: Bounds<Pixels>,
    scroll: f32,
) -> (Pixels, Pixels) {
    for row in &pre.rows {
        for piece in &row.pieces {
            if piece.byte.start <= byte && byte <= piece.byte.end {
                let dx = match &piece.kind {
                    PieceKind::Text(shaped) => shaped.x_for_index(byte - piece.byte.start),
                    PieceKind::Chip { .. } => px(0.),
                };
                return (
                    bounds.origin.x + px(piece.x) + dx,
                    bounds.origin.y + px(row.top) - px(scroll),
                );
            }
        }
    }
    // 落在所有行之后 → 文本末尾
    let y = pre
        .rows
        .last()
        .map(|r| bounds.origin.y + px(r.top) - px(scroll))
        .unwrap_or(bounds.origin.y);
    let x = pre
        .rows
        .last()
        .and_then(|r| r.pieces.last())
        .map(|p| bounds.origin.x + px(p.x + p.width))
        .unwrap_or(bounds.origin.x);
    (x, y)
}

/// 画一个实色矩形（选区 / 光标 / 下划线共用）。
fn paint_quad(window: &mut Window, x: Pixels, y: Pixels, w: Pixels, h: Pixels, color: Hsla) {
    window.paint_quad(gpui_kit::PaintQuad {
        bounds: Bounds::new(gpui_kit::point(x, y), gpui_kit::size(w.max(px(1.)), h)),
        corner_radii: gpui_kit::Corners::default(),
        background: color.into(),
        border_widths: gpui_kit::Edges::default(),
        border_color: color,
        border_style: gpui_kit::BorderStyle::Solid,
    });
}

// ── 输入：键盘 / 鼠标 / IME ────────────────────────────────────────────

impl ComposerState {
    /// 键盘：这里只处理**控制键**；字符与 IME 走 [`gpui_kit::EntityInputHandler`]。
    fn on_key_down(
        &mut self,
        ev: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        let ctrl = ks.modifiers.control;
        let shift = ks.modifiers.shift;
        let before = (self.buffer.text.clone(), self.buffer.cursor);
        match (ctrl, ks.key.as_str()) {
            (false, "enter") => {
                if shift {
                    self.buffer.insert("\n");
                } else {
                    cx.emit(ComposerEvent::Submit);
                }
            }
            (false, "backspace") => self.buffer.backspace(),
            (false, "delete") => self.buffer.delete(),
            (false, "left") => self.buffer.move_h(false, shift),
            (false, "right") => self.buffer.move_h(true, shift),
            (false, "home") => self.buffer.move_line_edge(false, shift),
            (false, "end") => self.buffer.move_line_edge(true, shift),
            (false, "up") => self.move_vertical(-1, shift),
            (false, "down") => self.move_vertical(1, shift),
            (true, "a") => self.buffer.select_all(),
            (true, "z") => {
                self.undo_once(cx);
                cx.stop_propagation();
                return;
            }
            (true, "c") => {
                let sel = self.buffer.selection();
                if sel.start != sel.end {
                    let text = self.buffer.text[sel].to_string();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                }
                cx.stop_propagation();
                return;
            }
            (true, "x") => {
                let sel = self.buffer.selection();
                if sel.start != sel.end {
                    let text = self.buffer.text[sel].to_string();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    self.buffer.delete_selection();
                }
            }
            // 粘贴：**自己接管**（平台的 paste 通道另有路径，两者共用
            // `handle_paste_item` 并用 200ms 去重）。早先只依赖平台 paste，
            // 实测「Ctrl+V 粘文字没反应」—— 两条路都兜住才不会漏。
            (true, "v") => {
                if let Some(item) = cx.read_from_clipboard() {
                    self.handle_paste_item(item, window, cx);
                }
                cx.stop_propagation();
                cx.notify();
                window.invalidate_character_coordinates();
                return;
            }
            _ => return,
        }
        if self.buffer.text != before.0 {
            self.undo.push(before);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        // 吞掉这些键：不让它们变成字符（字符走 IME 通道）
        cx.stop_propagation();
        window.invalidate_character_coordinates();
        cx.notify();
    }

    /// 上下移一行（按 `\n` 硬行做近似：同列优先，越界就贴行首/行尾）。
    fn move_vertical(&mut self, dir: i32, extend: bool) {
        let cursor = self.buffer.cursor;
        let start = self.buffer.line_start(cursor);
        let col = cursor - start;
        if dir < 0 {
            if start == 0 {
                self.buffer.set_cursor(0, extend);
                return;
            }
            let prev_end = start - 1;
            let prev_start = self.buffer.line_start(prev_end);
            let target = (prev_start + col).min(prev_end);
            self.buffer.set_cursor(target, extend);
        } else {
            let end = self.buffer.line_end(cursor);
            if end >= self.buffer.text.len() {
                self.buffer.set_cursor(self.buffer.text.len(), extend);
                return;
            }
            let next_start = end + 1;
            let next_end = self.buffer.line_end(next_start);
            let target = (next_start + col).min(next_end);
            self.buffer.set_cursor(target, extend);
        }
    }

    /// 鼠标按下：聚焦 + 定位光标（`Shift` 扩展选区）。
    fn on_mouse_down(
        &mut self,
        ev: &gpui_kit::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        let (ox, oy) = *self.origin.borrow();
        let x = f32::from(ev.position.x) - ox;
        let y = f32::from(ev.position.y) - oy;
        if let Some(pos) = self.hit_test(x, y) {
            self.buffer.set_cursor(pos, ev.modifiers.shift);
            // 从这一刻起进入拖拽态：后续 move 会把选区从按下点拉过去
            self.dragging = true;
        }
        cx.notify();
        window.invalidate_character_coordinates();
    }

    /// 鼠标移动：按住时**拖拽选择**（用户实测「不能按住拖动选文本」）。
    ///
    /// 锚点固定在按下点（set_cursor(.., true) 只在锚点为空时才设它），光标跟着走。
    fn on_mouse_move(
        &mut self,
        ev: &gpui_kit::MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging {
            return;
        }
        let (ox, oy) = *self.origin.borrow();
        let x = f32::from(ev.position.x) - ox;
        let y = f32::from(ev.position.y) - oy;
        if let Some(pos) = self.hit_test(x, y) {
            if self.buffer.anchor.is_none() {
                self.buffer.anchor = Some(self.buffer.cursor);
            }
            self.buffer.set_cursor(pos, true);
            cx.notify();
        }
    }

    /// 鼠标松开：结束拖拽（选区留在原地）。
    fn on_mouse_up(
        &mut self,
        _ev: &gpui_kit::MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dragging = false;
    }

    /// 当前组合态在 UTF-16 下的范围（平台对账用）。
    fn marked_utf16(&self) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| {
            utf16_from_byte(&self.buffer.text, m.start)..utf16_from_byte(&self.buffer.text, m.end)
        })
    }

    /// 用 `text` 替换一个 **UTF-16** 范围（清除组合态与选区）。
    fn replace_byte_range(
        &mut self,
        range_utf16: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.push_undo();
        let s = byte_from_utf16(&self.buffer.text, range_utf16.start);
        let e = byte_from_utf16(&self.buffer.text, range_utf16.end);
        self.buffer.anchor = None;
        self.buffer.text.replace_range(s..e, text);
        self.buffer.cursor = s + text.len();
        self.marked = None;
        cx.notify();
    }

    /// IME **组合中**：把上一次的组合串换成 `composing`（没有组合态就插入），
    /// 并把新区间记为组合态（绘制时加下划线、平台据此定位候选框）。
    fn apply_compose(&mut self, composing: &str, cx: &mut Context<Self>) {
        self.push_undo();
        match self.marked.take() {
            Some(m) => {
                self.buffer.text.replace_range(m.clone(), composing);
                self.buffer.cursor = m.start + composing.len();
                self.buffer.anchor = None;
            }
            None => self.buffer.insert(composing),
        }
        if !composing.is_empty() {
            let end = self.buffer.cursor;
            self.marked = Some(end.saturating_sub(composing.len())..end);
        }
        cx.notify();
    }

    /// IME **上屏**：把组合串换成最终文本（无组合态时等价于插入）。
    fn apply_commit(&mut self, text: &str, cx: &mut Context<Self>) {
        self.push_undo();
        match self.marked.take() {
            Some(m) => {
                self.buffer.text.replace_range(m.clone(), text);
                self.buffer.cursor = m.start + text.len();
                self.buffer.anchor = None;
            }
            None => self.buffer.insert(text),
        }
        cx.notify();
    }

    /// 处理一份剪贴板载荷：图片 → 交宿主；文件 → 行内 chip；其余 → 文本。
    ///
    /// `Ctrl+V`（键盘路径）与平台 `paste`（输入法框架路径）都会到这儿，用
    /// `last_paste` 做 200ms 去重，避免同一份内容被插两遍。
    pub(crate) fn handle_paste_item(
        &mut self,
        item: gpui_kit::ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(t) = self.last_paste {
            if t.elapsed().as_millis() < 200 {
                return;
            }
        }
        self.last_paste = Some(std::time::Instant::now());
        // 分流顺序不可"优化"：图片优先（走附件区），文件其次（行内 chip），最后文本。
        // 反过来会把「复制的 png」变成模型读不了的路径引用（Web 版同一条结论）。
        let has_image = item
            .entries
            .iter()
            .any(|e| matches!(e, gpui_kit::ClipboardEntry::Image(_)));
        if has_image {
            cx.emit(ComposerEvent::PasteImage(item));
            return;
        }
        let paths: Vec<String> = item
            .entries
            .iter()
            .filter_map(|e| match e {
                gpui_kit::ClipboardEntry::ExternalPaths(p) => Some(
                    p.paths()
                        .iter()
                        .map(|x| x.to_string_lossy().to_string())
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect();
        if !paths.is_empty() {
            self.push_undo();
            for p in paths {
                self.buffer.insert_file_ref(&p);
            }
            cx.notify();
            return;
        }
        if let Some(text) = item.text() {
            self.replace_text_in_range(None, text.as_str(), window, cx);
        }
    }

    /// 模拟一次 IME 组合 → 上屏（**验收用**：真实输入法只能在真机上敲，
    /// 但「组合串是否被替换」这条逻辑可以用它断言 —— 用户实测报的
    /// 「每敲一个字母都追加」正是这里出错）。
    pub(crate) fn ime_probe(&mut self, cx: &mut Context<Self>) {
        for composing in ["w", "wo", "wǒ"] {
            self.apply_compose(composing, cx);
        }
        self.apply_commit("我", cx);
    }
}

/// UTF-16 长度（IME 与 GPUI 的索引单位）。
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// UTF-16 索引 → 字节索引（落在代理对中间时向下取整）。
fn byte_from_utf16(text: &str, pos: usize) -> usize {
    let mut count = 0usize;
    for (byte, ch) in text.char_indices() {
        if count >= pos {
            return byte;
        }
        count += ch.len_utf16();
    }
    text.len()
}

/// 字节索引 → UTF-16 索引。
fn utf16_from_byte(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    let mut count = 0usize;
    for (b, ch) in text.char_indices() {
        if b >= byte {
            break;
        }
        count += ch.len_utf16();
    }
    count
}

impl gpui_kit::EntityInputHandler for ComposerState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let s = byte_from_utf16(&self.buffer.text, range_utf16.start);
        let e = byte_from_utf16(&self.buffer.text, range_utf16.end);
        *adjusted_range = Some(range_utf16);
        Some(self.buffer.text[s..e].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui_kit::UTF16Selection> {
        let sel = self.buffer.selection();
        let start = utf16_from_byte(&self.buffer.text, sel.start);
        let end = utf16_from_byte(&self.buffer.text, sel.end);
        Some(gpui_kit::UTF16Selection {
            range: start..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| {
            utf16_from_byte(&self.buffer.text, m.start)..utf16_from_byte(&self.buffer.text, m.end)
        })
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 平台的「提交」语义：有显式 range 就替换它；**没有 range 但正处在组合态时，
        // 要替换掉组合串**（这一步错了就是用户实测的「每敲一个字母都追加一遍」：
        // w → wo → wǒ 会累积成 `wwowǒ`）。都没有才是普通插入。
        match range_utf16 {
            Some(r) => self.replace_byte_range(r, text, cx),
            None => self.apply_commit(text, cx),
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 组合态更新：平台通常把 range 传成 `None`（意思是「就是刚才那段 marked」），
        // 所以这里一律**替换上一次的组合串**，而不是插入。
        if let Some(r) = range_utf16 {
            if Some(&r) != self.marked_utf16().as_ref() {
                // 平台的 range 与本地记录不一致（少见）：先按平台给的替换掉
                self.replace_byte_range(r, "", cx);
            }
        }
        self.apply_compose(new_text, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // IME 候选框位置：用该范围的起点（光标）近似
        let byte = byte_from_utf16(&self.buffer.text, range_utf16.start);
        let (ox, oy) = *self.origin.borrow();
        let _ = element_bounds;
        let hits = self.hits.borrow();
        let mut y = oy;
        for row in hits.iter() {
            if row
                .pieces
                .iter()
                .any(|p| p.byte.start <= byte && byte <= p.byte.end)
            {
                y = oy + row.top - self.scroll;
                break;
            }
            y = oy + row.top + row.height - self.scroll;
        }
        Some(Bounds::new(
            gpui_kit::point(px(ox), px(y)),
            gpui_kit::size(px(1.), px(18.)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui_kit::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (ox, oy) = *self.origin.borrow();
        let x = f32::from(point.x) - ox;
        let y = f32::from(point.y) - oy;
        let byte = self.hit_test(x, y)?;
        Some(utf16_from_byte(&self.buffer.text, byte))
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(utf16_len(&self.buffer.text))
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }
    fn paste(
        &mut self,
        item: gpui_kit::ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 平台粘贴路径（输入法框架/系统菜单）：与 Ctrl+V 共用同一份分流逻辑
        self.handle_paste_item(item, window, cx);
    }
}
