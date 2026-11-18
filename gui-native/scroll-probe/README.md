# scroll-probe — 消息列表滚动语义的回归回路

给 `gui-native/vendor/gpui-pre`（`list.rs` 的 21 处 `[天演 patch]`）提供一条**能跑**的
自动化验证。GUI 层面测不到这些：引擎的滚动/跟随/测量状态在 `ListState` 内部，
人肉点界面无法判定「1px 的上滚到底有没有被吞掉」「位置是不是被某帧悄悄挪了」。

```powershell
# 首次会完整编译一次 gpui-pre（含 test-support），之后是增量
cargo test --manifest-path gui-native\scroll-probe\Cargo.toml
```

## 它锁住的四条

| 用例 | 用户现象 |
|------|----------|
| `one_pixel_scroll_up_does_not_re_engage_follow` | 想往上看就被弹回底部（滚轮/触控板平滑后的第一帧增量只有 1px） |
| `streaming_growth_does_not_yank_user_back_to_tail` | 流式输出中上滚看历史，被每个新 delta 拽回底部 |
| `follow_tail_keeps_viewport_filled_when_tail_grows` | 贴底时最后一条正在增长，视口下方留白 |
| `no_measurement_noise_when_items_already_measured` | 内容没变却「跳一下」（高度重算把位置挪了） |

改动 `list.rs` 的任何 patch、或 `app.rs` 里 `new_msg_list_state` /
`scroll_to_end_if_following` / `follows_tail` / 各条 `splice` 路径，都要跑一次。

判别力说明（别把绿当成「已覆盖」）：

- 只有第 1 条是**被反向验证过**的 —— 把 `list.rs` 的「上滚 ⇒ 停跟随 / 到末尾 ⇒
  跟随」两条判断拆回两个独立 `if`，它必红（2026-11-18 那次的定案证据）；
- 另外三条在当前实现下是绿的，锁的是**已经修对的行为**（回归护栏）。其中第 2 条在
  上滚 300px 的旧实现下也是绿的（滚得够多就踩不到那条 1px 判据）——**不能**拿它们
  当「问题已修复」的证明。

## 构造口径必须与 app 一致

`build()` 里那 4 行就是 `app::new_msg_list_state` 的全部：`Top` + `400px overdraw`
+ `FollowMode::Tail` + `seed_viewport_height`。**刻意不做** `measure_all()`、不给
未测量项估值 —— 那两件事会让 `summary().height` 在「估值/真值」之间跳，是抖动的
来源（详见 `app.rs` 该函数文档）。用例里若要模拟「新项未测量」，直接 `splice` 即可。

## 为什么单独一个 workspace

- 不进 `gui-native`：那是单一根包 + 一套 MSI 打包脚本，多一个成员会给 `cargo build`
  和打包脚本引入无关的编译目标；
- 不进主 workspace：它要 `test-support` 特性，会把 `proptest`、`wayland`、`x11`
  一串测试依赖拉进内核构建图。

## 诊断日志

运行时排查用 `gui-native/src/scroll_diag.rs`（默认开启，产物在
`%LOCALAPPDATA%\tianyan\logs\gui-scroll.log`，超过 2 MB 轮转一次）。它与本目录
互补：这里锁**语义**，日志抓**现场**。
