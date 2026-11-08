# gpui-pre 本地修改清单（天演 patch）

本目录是 `gpui-pre 0.3.6` 的 vendor 副本，经 `gui-native/Cargo.toml` 的
`[patch.crates-io]` 接入。**上游 crates.io 版本被替换成了这份代码** ——
它不再是「可随时 `cargo update` 的依赖」，而是一份需要维护的分叉。

全部修改集中在 **`src/elements/list.rs`**，共 **21 处**，均以 `/// [天演 patch]`
或 `// [天演 patch]` 注释标记。检索：

```powershell
Select-String -Path (Get-ChildItem -Recurse gui-native\vendor\gpui-pre -Filter *.rs |
  ForEach-Object FullName) -Pattern '\[天演 patch\]'
```

> 行号会随任何一次编辑漂移，**以 marker 为准**，下表行号仅对应
> `1.2.5`（2026-11-08）的 vendor 快照。

## 为什么必须 vendor

三条上游缺陷，任意一条都能让「切回会话位置」失效，且**修不了在应用层**：

1. `List::request_layout` 在拿到真实宽度之前会先跑一次
   `layout_items(available_width = None, …)`。宽度未知时元素按 `MaxContent`
   排版、**不换行**，一条长消息量出来的高度只有真实值的零头
   （实测同一条：**66px** / **450px** vs 真实 **2156px**）。这个脏高度一旦写进
   测量缓存，下一帧就会直接命中它而不再重测。
2. `layout_items` 里「下方不足一屏就把位置重算成底部对齐」那条归一化，对**任何**
   输入都会动手，把用户正常的阅读位置改成贴底；「是否恢复跟随」同样在布局层
   按 `total_height` 判，而 `total_height` 会随测量变化 —— 两者叠加即
   「切回会话跳到底部」。
3. `scroll()` 里 `(scroll_top - delta).min(scroll_max)` 是**先减后钳**：跟随态下
   `scroll_top` 是「末尾之后」`(count, 0)`，其绝对像素是总高，比 `scroll_max =
   总高 − 视口高` 还大，于是任何上滚都被钳回贴底，用户滚一格纹丝不动。

## 21 处清单

### 状态字段与 API（新增）

| 行 | 位置 | 内容 |
|----|------|------|
| 76 | `ListState::suppress_autoscroll` | 恢复阅读位置那一帧压掉子元素发来的 autoscroll 请求（焦点恢复会把视口从 `(150, 776)` 拽到 `(147, 9.3)` 并顺手开启跟随） |
| 86 | `ListState::measure_all_requested` | 全量测量请求标记。单存一份是因为 `prepaint` 里「宽度变化 ⇒ `measuring_behavior.reset()`」会把 `Measure(false)` 打回 `Visible`，`measure_all()` 因此形同虚设 |
| 344 | `List::measure_all` | 同时立请求标记 |
| 352 | 新增 `List::remeasure_all` | 在**已有实例**上重新启用全量测量。`restore_view_state` 复用快照里的 `ListState`（为了带 `last_layout_bounds`），其 `measuring_behavior` 通常已是「已测过」，新布局不会全量测量 ⇒ 用户一滚动，未测项逐个被量、累计高度变化、位置漂 |
| 373 | 新增 `List::seed_viewport_height` | 给新实例预置视口高。首帧没有 `last_layout_bounds` 时会拿 `overdraw`（2400px）当视口高，「贴底」被算成 `总高 − 2400`，用户看到**第一次打开会话闪一下**。宽度留 0 是故意的：不改测量口径，只让 `available_height` 一开始就是真值 |
| 447 | 新增 `List::set_uniform_item_height` | 给 `splice` 后的未测量项补高度 hint。`splice` 把新项建成 `Unmeasured { size_hint: None }`，`ListItemSummary::height` 对 `None` 计 **0px** ⇒ 累计高度严重偏小 ⇒ 跨过这些项时跳好几条（实测「上滚到用户消息附近跳过去 / 速度突然变快」） |
| 462 | 新增 `List::average_item_height` | 已测量项的**真实平均**高度，供应用层做估值。TanStack Virtual 的结论是「估值宁可偏大，估小必然在测量后展开从而跳跃」；原来固定 280px 对动辄 2000+ 的消息是系统性低估 |
| 739 | 新增 `List::suppress_autoscroll_once` | 一次性的抑制入口（应用后立即复位，之后正常交互不受影响） |

### `scroll()`（第 1013 行起）

| 行 | 内容 |
|----|------|
| 1013 | **先钳到 `scroll_max` 再减 delta**。原式先减后钳 ⇒ 任何上滚都被钳回贴底 ⇒ 「上滚一格纹丝不动 + 每帧 `start_following` 又把它设回跟随」= 往返跳 |
| 1031 | **逐项行走**替换 `items.find(&Height(new_scroll_top))`。后者把落点建立在**全表**累计高度上，任何一项不准（未测量项按 `size_hint` 甚至 0 计）误差都会线性放大；逐项行走只依赖「当前项到目标项」这一小段，最坏是一条的估值差。位置表达仍是逻辑坐标 `(item_ix, offset_in_item)`，渲染侧语义不变 |
| 1117 | **跟随状态只由用户滚动驱动**：上滚 ⇒ 停跟随；滚到 `scroll_max` ⇒ 重新跟随。原判据在布局层按 `scroll_offset + available >= total − 1`，而 `total` 会随测量变化 |

### `layout_items()`

| 行 | 内容 |
|----|------|
| 1232 | `store_measurement`：**宽度未知时完全不更新缓存**。早先写 `Unmeasured { size_hint }` 会制造每帧 `Measured → Unmeasured → Measured` 循环，`summary().height` 跟着抖，总高一变小还会误触发「已回到尾部」判据 |
| 1349 | 「向上补齐」只在**宽度已定**时做（`available_width.is_some()`） |
| 1359 | 追加**输入自洽**门槛：`offset_in_item <= rendered_height`。`offset_in_item` 是「视口顶部落在锚点项内部多深」，不可能超过「锚点到末尾的累计高度」——超过了说明高度是脏的或位置越界，此时保持输入原样 |
| 1367 | 追加**锚点已测量**门槛。`rendered_height` 里未测量项按 `size_hint` 计（`splice` 新项甚至 0），首帧锚点常常还没量出来（实测 `ih` 从 0 → 177 → 2156 要三帧），此时「下方不足一屏」成立 ⇒ 位置被补齐 ⇒ 下一帧整表贴底 |
| 1382 | **归一化只服务一种输入**：`item_ix >= count`（「末尾之后」`(count, 0)`，`scroll_to_end()` 留下的贴底意图）。正常滚动不需要它 —— `scroll()` 已有 `.min(scroll_max)` 钳制，用户滚不出内容末尾 |
| 1467 | **删除布局尾部的自动恢复跟随**。三道门槛（宽度已知 / 锚点已测 / 锚点必须是最后一条）都挡不住：最后一条消息往往就是很长的那条（长回复 / 工具卡片），用户滚到它**内部**时锚点仍是最后一条 |

### `request_layout()` / `prepaint()`

| 行 | 内容 |
|----|------|
| 1737 | 首帧 fallback 视口高由 `overdraw` 改为 `overdraw.min(600px)`。`overdraw` 是「上下各预测量多少」的参数，不是视口高 |
| 1819 | `prepaint` 里「宽度变化 ⇒ invalidate 所有项高度」时，`size_hint` 用 `item.size_hint()` 而**不是 `None`**。原来是 `None` ⇒ 总高在重测之前直接归零 ⇒ 「是否已回到尾部」必然算错。保留旧高度当估算值，项一经渲染就被真值替换 |
| 1834 | `measure_all_requested` 在**宽度检查之后**重新生效（宽度检查会把 `remeasure_all()` 设下的 `Measure(false)` 冲掉） |
| 1845 | 取走并复位 `suppress_autoscroll`，传给 `prepaint_items` |

## 应用层的对应面（不在 vendor 里）

patch 只保证「引擎不主动改坏位置」，跟随语义的另一半在应用层：

- `msg_scrolled_up` 标记 + `scroll_to_end_if_following`（应用层自己判跟随）
- `restore_view_state` 复用快照 `ListState` + `suppress_autoscroll_once()`
- 估值 1200px 起步、`average_item_height()` 修正、`splice` 后 `set_uniform_item_height` +
  `remeasure_all`（`load_older` 路径）
- `overdraw` 2400px（gpui 文档：overdraw 就是为「滚动时不闪烁」而存在）

## 升级 gpui-pre 的流程（必做）

1. 解包新版到临时目录，与当前 `vendor/gpui-pre` 逐文件 diff（`src/elements/list.rs`
   是唯一有 patch 的文件，重点看它）。
2. 把 21 处 marker **逐条平移**到新版。上游可能已修掉其中若干条 —— 确认上游实现
   等价后再删，**不要因为「看起来像修好了」就整段丢掉**。
3. 跑行为自测（顺序无关，全部通过才算同步完成）：
   - 切回一个**上滚过**的会话：位置不跳、不贴底、不闪
   - 滚到长回复**内部**再切走切回：位置保持
   - 向上滚动经过高元素 / 用户消息：不跳过、不往返跳
   - 首次打开会话：不闪（首帧不落到 `总高 − overdraw`）
   - 流式输出中上滚：不被拽回底部；回到底部后继续跟随
4. 门禁：`pwsh -File gui-native\…` 的 fmt / clippy --all-targets / `cargo test`
   （gui-native 73 个单测）+ 一次 `build-msi.ps1` 出包冒烟。

**漏同步的后果是静默的**：编译照过、测试照绿（单测覆盖不到滚动引擎内部），
只有用户切回会话时才表现为「又跳了」。
