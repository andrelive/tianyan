//! UI 侧验证（阶段 A）：
//! 1. 虚拟列表扛 5000 条消息时的渲染行为（关键指标：render_item 被调用多少次）
//! 2. 代码编辑器开大文件（tree-sitter 高亮 + 滚动）

use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::{IndexPath, Root};
use gpui_kit::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const LIST_COUNT: usize = 1000;
const CODE_LINES: usize = 100_000;

/// 模拟聊天列表：变长文本，验证虚拟化 + 高度测量
struct ChatDelegate {
    count: usize,
    calls: Arc<AtomicUsize>,
}

impl ListDelegate for ChatDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &gpui_kit::App) -> usize {
        self.count
    }

    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let row = ix.row;
        let text = if row % 7 == 0 {
            format!(
                "第 {} 条 · 这条是长消息，用于检验变高测量：包含中文与 English 123，以及一些标点。",
                row + 1
            )
        } else {
            format!("第 {} 条消息", row + 1)
        };
        Some(ListItem::new(row).h(px(52.)).child(div().px_3().child(text)))
    }
}

/// 生成一份用于编辑器压测的 Rust 代码
fn gen_code(lines: usize) -> String {
    let mut s = String::with_capacity(lines * 48);
    s.push_str("// 自动生成的压测代码\n");
    for i in 0..lines {
        match i % 5 {
            0 => s.push_str(&format!("fn handler_{i}(input: &str) -> usize {{\n")),
            1 => s.push_str(&format!("    let n = input.len() + {i};\n")),
            2 => s.push_str("    // 中英混排注释 comment\n"),
            3 => s.push_str(&format!("    println!(\"value = {{}}\", {i});\n")),
            _ => s.push_str("    n\n}\n"),
        }
    }
    s
}

struct DemoApp {
    chat: Entity<ListState<ChatDelegate>>,
    editor: Entity<EditorState>,
}

impl DemoApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let chat = cx.new(|cx| {
            ListState::new(
                ChatDelegate {
                    count: LIST_COUNT,
                    calls: calls.clone(),
                },
                window,
                cx,
            )
        });

        let code = gen_code(CODE_LINES);
        println!(
            "[编辑器] 待加载代码 {} 行 / {:.1} MB",
            CODE_LINES,
            code.len() as f64 / 1024.0 / 1024.0
        );
        let t0 = std::time::Instant::now();
        let editor = cx
            .new(|cx| EditorState::new(window, cx).language("rust").default_value(&code));
        println!("[编辑器] EditorState 创建耗时 {:?}", t0.elapsed());

        // 每 2 秒报告一次 render_item 累计调用数：
        // 远小于总条数 = 虚拟化生效；与总条数同量级 = 虚拟化失效
        cx.spawn(async move |_this, cx| {
            let mut last = 0usize;
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(2))
                    .await;
                let now = calls.load(Ordering::Relaxed);
                println!(
                    "[列表] render_item 累计 {} 次（本轮新增 {}）/ 总条数 {}",
                    now,
                    now - last,
                    LIST_COUNT
                );
                last = now;
            }
        })
        .detach();

        Self { chat, editor }
    }
}

impl Render for DemoApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                // 左：虚拟列表
                div()
                    .w(px(420.))
                    .h_full()
                    .border_r_1()
                    .child(List::new(&self.chat)),
            )
            .child(
                // 右：代码编辑器
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(Editor::new(&self.editor).h_full()),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1360.), px(860.)), cx);
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("天演 · UI 侧验证（虚拟列表 + 代码编辑器）".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| DemoApp::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });
}
