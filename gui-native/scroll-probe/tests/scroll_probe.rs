//! 复现探针：流式输出期间用户上滚后的滚动位置。
//!
//! 用例对应 gui-native 的真实调用序列与用户现象：
//! 1. `one_pixel_scroll_up_does_not_re_engage_follow`
//!    —— 滚轮平滑后首帧增量 1px，也必须判成「用户要离开底部」；
//! 2. `streaming_growth_does_not_yank_user_back_to_tail`
//!    —— 上滚后每个 delta 内容变高，位置必须不动；
//! 3. `follow_tail_keeps_viewport_filled_when_tail_grows`
//!    —— 跟随态下末条快速增长，视口必须始终被内容填满（不许底部留白）；
//! 4. `no_measurement_noise_when_items_already_measured`
//!    —— 已测量项在内容不变时，布局不得改动位置（抖动护栏）。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, list, point, px, size, AppContext, Context, Element, Entity, FollowMode, IntoElement,
    ListState, Render, ScrollDelta, ScrollWheelEvent, Styled, TestAppContext, VisualTestContext,
    Window,
};

const VIEW_H: f32 = 400.;
/// 与 `app::MSG_LIST_OVERDRAW` 同值（官方 `MessageScroller` 的 `LIST_OVERDRAW`）。
const OVERDRAW: f32 = 400.;

/// 与 gui-native 的 `new_msg_list_state` 同口径：
/// Top 对齐 + 400px overdraw + Tail 跟随 + 预置视口高，**不**全量测量、**不**给估值。
fn build(
    cx: &mut VisualTestContext,
    tail_h: Rc<Cell<usize>>,
) -> (ListState, Entity<TestView>) {
    let state = ListState::new(12, gpui::ListAlignment::Top, px(OVERDRAW));
    state.set_follow_mode(FollowMode::Tail);
    state.seed_viewport_height(px(VIEW_H));
    let s = state.clone();
    let view = cx.update(|_, cx| cx.new(|_| TestView { state: s, tail_h }));
    (state, view)
}

struct TestView {
    state: ListState,
    tail_h: Rc<Cell<usize>>,
}

impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let tail = self.tail_h.get();
        list(self.state.clone(), move |ix, _, _| {
            let h = if ix == 11 { tail } else { 100 };
            div().h(px(h as f32)).w_full().into_any()
        })
        .w_full()
        .h_full()
    }
}

fn draw(cx: &mut VisualTestContext, view: &Entity<TestView>) {
    let v = view.clone();
    cx.draw(point(px(0.), px(0.)), size(px(300.), px(VIEW_H)), |_, _| {
        v.into_any_element()
    });
}

/// 跟随态下**逐帧小增量**（流式回答的真实节奏：每帧几十像素）：视口起点必须
/// 单调向上推进，且每帧都严格贴底。
///
/// 为什么要单独一条：另外三条用的是「跳着涨」的 tail 高度（100 → 250 → 900），
/// 而用户报的「流式输出时跳」发生在**每帧都在长**的那段时间里。帧内位置一旦
/// 来回翻（例如「末尾之后」与「贴底」两种表示交替），视觉上就是上下抖动。
#[gpui::test]
fn follow_tail_smooth_streaming_growth_is_monotonic(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let tail_h = Rc::new(Cell::new(200usize));
    let (state, view) = build(cx, tail_h.clone());
    draw(cx, &view);

    let mut prev_top: Option<f32> = None;
    // 末条从 200px 长到 3200px，每帧 +37px（≈ 两行正文）
    let mut step = 200usize;
    while step <= 3200 {
        tail_h.set(step);
        draw(cx, &view);
        let top = state.logical_scroll_top();
        let top_px = f32::from(px(top.item_ix as f32 * 100.) + top.offset_in_item);
        let total = 11. * 100. + step as f32;
        assert!(
            top_px + VIEW_H <= total + 0.5,
            "tail={step}：视口越过了内容末尾（下方留白）top={top:?} top_px={top_px}"
        );
        assert!(
            top_px + VIEW_H >= total - 0.5,
            "tail={step}：没有贴到底 top={top:?} top_px={top_px}"
        );
        if let Some(prev) = prev_top {
            assert!(
                top_px >= prev - 0.5,
                "tail={step}：内容只增不减，视口起点却回退了 {prev} -> {top_px}"
            );
        }
        prev_top = Some(top_px);
        step += 37;
    }
}

#[gpui::test]
fn one_pixel_scroll_up_does_not_re_engage_follow(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let tail_h = Rc::new(Cell::new(100usize));
    let (state, view) = build(cx, tail_h);
    draw(cx, &view);
    assert!(state.is_following_tail(), "首帧应当贴底跟随");
    let bottom = state.logical_scroll_top();
    let bottom_px = px(bottom.item_ix as f32 * 100.) + bottom.offset_in_item;

    // 滚轮/触控板经平滑后的**第一帧增量可能只有 1px**。旧实现在这里翻车：
    // 先 `stop_following()`，紧接着「已在末尾附近」（`new >= scroll_max - 1`）
    // 那条判断又 `start_following()` 把它盖回去 —— 上滚的第一小步被吞掉，
    // 下一帧整表贴底，用户感觉「一想往上看就弹回底部」。
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(150.), px(200.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(1.))),
        ..Default::default()
    });
    assert!(
        !state.is_following_tail(),
        "上滚 1px 也必须判定为「用户要离开底部」"
    );
    draw(cx, &view);
    assert!(
        !state.is_following_tail(),
        "布局阶段不得把它改回跟随（那等于整表贴底）"
    );
    let after = state.logical_scroll_top();
    let after_px = px(after.item_ix as f32 * 100.) + after.offset_in_item;
    assert_eq!(
        after_px,
        bottom_px - px(1.),
        "上滚 1px 后位置应恰好上移 1px：{bottom:?} -> {after:?}"
    );
}

#[gpui::test]
fn streaming_growth_does_not_yank_user_back_to_tail(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let tail_h = Rc::new(Cell::new(100usize));
    let (state, view) = build(cx, tail_h.clone());
    draw(cx, &view);
    assert!(state.is_following_tail(), "首帧应当贴底跟随");

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(150.), px(200.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(300.))),
        ..Default::default()
    });
    draw(cx, &view);
    assert!(!state.is_following_tail(), "上滚后必须停止跟随");
    let before = state.logical_scroll_top();

    for step in [300usize, 600, 900] {
        tail_h.set(step);
        draw(cx, &view);
    }

    assert!(
        !state.is_following_tail(),
        "流式增长不得重新开启跟随（那会把用户拽回底部）"
    );
    let after = state.logical_scroll_top();
    assert_eq!(
        (before.item_ix, before.offset_in_item),
        (after.item_ix, after.offset_in_item),
        "流式增长期间用户的上滚位置被改动：{before:?} -> {after:?}"
    );
}

#[gpui::test]
fn follow_tail_keeps_viewport_filled_when_tail_grows(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let tail_h = Rc::new(Cell::new(100usize));
    let (state, view) = build(cx, tail_h.clone());

    for step in [100usize, 250, 900, 1400] {
        tail_h.set(step);
        draw(cx, &view);

        let top = state.logical_scroll_top();
        let total = px(11. * 100. + step as f32);
        let top_px = px(top.item_ix as f32 * 100.) + top.offset_in_item;
        assert!(
            top_px + px(VIEW_H) <= total + px(1.),
            "tail={step}：视口底部越过内容末尾（下方留白），top={top:?} total={total:?}"
        );
        assert!(
            top_px + px(VIEW_H) >= total - px(1.),
            "tail={step}：没有贴到底，top={top:?} total={total:?}"
        );
    }
}

/// 抖动护栏：内容没变时，连布局几帧位置必须完全不动。
///
/// 用户报的「跳跃」多是这一类：某一帧的高度重算把 `summary().height` 改了，
/// 位置被钳一下又回来。这条断言把「静止即稳定」变成可执行判据。
#[gpui::test]
fn no_measurement_noise_when_items_already_measured(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let tail_h = Rc::new(Cell::new(200usize));
    let (state, view) = build(cx, tail_h);

    // 先上滚到中间，离开「贴底」这条特殊路径。
    draw(cx, &view);
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(150.), px(200.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(500.))),
        ..Default::default()
    });
    draw(cx, &view);
    let first = state.logical_scroll_top();
    let first_px = px(first.item_ix as f32 * 100.) + first.offset_in_item;

    for _ in 0..5 {
        draw(cx, &view);
        let now = state.logical_scroll_top();
        let now_px = px(now.item_ix as f32 * 100.) + now.offset_in_item;
        assert_eq!(
            now_px, first_px,
            "内容没变但布局把位置挪了：{first:?} -> {now:?}"
        );
    }
}
