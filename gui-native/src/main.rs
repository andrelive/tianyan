use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::text::{TextView, TextViewMotion, TextViewState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::Root;
use gpui_kit::*;
use std::time::Duration;

/// 模拟一次 LLM 回复的完整内容（含中文、代码块、列表、行内样式）
const STREAM_SOURCE: &str = r#"## 流式渲染验证

这是模拟 LLM 逐字输出的内容。它以约 40 字符/秒的速度追加进来，请观察：

1. 文字是否**平滑出现**（`stream_fade` 渐显），而不是整块跳变
2. 追加过程中**滚动条是否跟随到底部**，而不是停在原位或被拉回顶部
3. 代码块在**围栏尚未闭合**的中间态是什么表现
4. 中文与 English 123 混排时**换行位置**是否稳定，有没有半个字被切

```rust
fn stream_token(delta: &str, cx: &mut Context<Self>) {
    self.state.update(cx, |state, cx| state.push_str(delta, cx));
}
```

| 检查项 | 期望 |
|---|---|
| 渐显 | 新字有短暂淡入 |
| 滚动 | 自动跟随底部 |

> 最后一行：流到这里说明整段追加完毕，且没有出现撕裂或重复。
"#;

struct App {
    md: Entity<TextViewState>,
    input: Entity<TextareaState>,
}

impl App {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 流式文档：初始为空，靠 push_str 逐字追加。
        // 注意：TextViewMotion 默认全部为 ZERO（= 立即整块出现），必须显式开启渐显
        let md = cx.new(|cx| {
            let mut state = TextViewState::markdown("", cx);
            state.set_motion(
                TextViewMotion::default()
                    .with_stream_fade(Duration::from_millis(320))
                    .with_stream_fade_stagger(Duration::from_millis(35)),
            );
            state
        });
        let input = cx.new(|cx| TextareaState::new(window, cx));

        cx.observe(&input, |_this, input, cx| {
            let text = input.read(cx).value().to_string();
            println!("[输入] {}", text.replace('\n', "\\n"));
        })
        .detach();

        // 模拟 SSE：循环重播，便于观察流式过程。每轮内容 = STREAM_SOURCE x3（约 1290 字符），
        // 每 25ms 追加 4 个字符（接近真实 LLM 流式节奏）
        let chunks: Vec<char> = STREAM_SOURCE.repeat(3).chars().collect();
        let total = chunks.len();
        let md_task = md.clone();
        cx.spawn(async move |_this, cx| {
            let mut round: u32 = 0;
            loop {
                round += 1;
                cx.background_executor()
                    .timer(Duration::from_millis(1200))
                    .await;
                let started = std::time::Instant::now();
                let mut slow = 0u32;
                let mut worst = Duration::ZERO;
                let mut i = 0usize;
                while i < total {
                    cx.background_executor()
                        .timer(Duration::from_millis(25))
                        .await;
                    let end = (i + 4).min(total);
                    let burst: String = chunks[i..end].iter().collect();
                    i = end;
                    cx.update(|app| {
                        let t0 = std::time::Instant::now();
                        md_task.update(app, |state, cx| {
                            state.push_str(&burst, cx);
                            // 流式跟随：追加后把视图滚到底（聊天场景必需）
                            state.list_state().scroll_to_end();
                        });
                        let dt = t0.elapsed();
                        if dt > worst {
                            worst = dt;
                        }
                        // 5ms 阈值：单次追加超过它说明重解析开始拖后腿
                        if dt.as_millis() >= 5 {
                            slow += 1;
                        }
                    });
                }
                println!(
                    "[stream] 第 {round} 轮完成：{total} 字符，兡时 {:?}，单次最大 {worst:?}，慢帧 {slow} 次",
                    started.elapsed()
                );
                cx.background_executor()
                    .timer(Duration::from_millis(2500))
                    .await;
                cx.update(|app| {
                    md_task.update(app, |state, cx| state.set_text("", cx));
                });
            }
        })
        .detach();

        Self { md, input }
    }
}

impl Render for App {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p_4()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(TextView::new(&self.md).scrollable(true)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .pt_2()
                    .child(Textarea::new(&self.input).h(px(80.)).w_full()),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1180.), px(820.)), cx);
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("天演 · 流式渲染验证".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| App::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });
}
