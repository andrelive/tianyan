# 依赖升级台账（期 5）

> 目的：记录**每个依赖升级批次的决策与依据**，尤其是「为什么不升」——避免将来重复
> 调研同一问题，也避免「无脑升到最新」引入无收益的风险。
> 基线：`0.8.0`（ADR-049 期 1–3 的微内核化重构完成后）。**该版本号未对外发布**（无 tag、无产物），
> 其全部内容随 **1.0.0** 一并交付；目标：1.0 前完成依赖现代化。

---

## 批次总览

| 批 | 范围 | 状态 | 提交 |
|---|---|---|---|
| **P0** | Rust：`cargo update`（semver 内 360 包） | ✅ | `386bd8f` |
| **F1** | 前端：`npm update`（semver 内 118 包） | ✅ | `386bd8f` |
| **P1** | `thiserror 2` / `rusqlite 0.33` / `image 0.25` / `calamine 0.28` / `config 0.15` / `dirs 6` | ✅ | `a0a9831` |
| **P2** | `pulldown-cmark 0.9 → 0.13` | ✅ | `dbd3245` |
| **P3** | `schemars 0.8 → 1.0` | ✅ | `8398257` |
| **P4** | `lancedb 0.33 → 0.39/0.40` | ⛔ **暂缓**（见下） | — |
| **F2** | `vite 8`（Rolldown）/ `vitest 5` / `plugin-react 6` / `jsdom 30` | ✅ | `80decfe` |
| **F3** | `React 19` / `zustand 5` | ✅ | 本批 |
| **F3′** | `msw 3` / `Tailwind 4` | ⛔ **暂缓**（见下） | — |

---

## P4：为什么 `lancedb` 停在 0.33

**结论：暂缓升级，不是「没时间」，是上游有编译缺陷 + 收益为零。**

### 现象

升到 `lancedb 0.39.0` 或 `0.40.0`（最新），编译失败，错误在 **lancedb 自己的源码**里：

```
error[E0599]: no variant named `Http` found for enum `error::Error`
  --> lancedb-0.39.0/src/job.rs:102:40
```

### 根因（已核对源码）

- `lancedb/src/error.rs:113` 的 `Http { ... }` 变体带 **`#[cfg(feature = "remote")]`**；
- 但 `lancedb/src/lib.rs:188` 的 `pub mod job;` **没有** 对应的 `#[cfg(feature = "remote")]`；
- `job.rs` 在两处无条件构造 `Error::Http`。

于是**任何未启用 `remote` feature 的构建都会编译失败**——这是上游打包缺陷，不是我们的用法问题
（0.39 与 0.40 都是同一处，说明跨版本未修）。

### 为什么不干脆启用 `remote`

`remote` feature 会引入：`arrow-flight` + `prost` + `reqwest` + `http` + `tonic`(gRPC) +
`urlencoding` + `lance-namespace-impls/rest*` —— 一整套**远程 LanceDB 服务客户端**依赖。

本项目只用**本地嵌入式** LanceDB（`connect(path)` + 本地表），这些依赖一行都用不到，纯增加
编译时间与二进制体积。

### 收益 vs 成本

我们用到的 lancedb API 面很窄：`connect` / `create_table` / `open_table` / `merge_insert` /
`vector_search` / `delete`。0.33 已完全满足；0.39/0.40 的新特性（namespace / remote / REST）
与本项目无关。**收益 ≈ 0，成本 = 重依赖 + 上游缺陷风险。**

### 重新评估的触发条件（任一满足）

1. 上游修复 `job.rs` 的 cfg gate（可先用 `cargo check` 验证 0.4x 是否仍报 `Error::Http`）；
2. 本项目需要远程 LanceDB（多机共享向量库）或 namespace 特性；
3. 0.33 出现安全漏洞（需查 RUSTSEC 公告）。

> 现状：`Cargo.toml` 的 `lancedb` 保持 `"0.33"`，并在该行上方留了注释说明原因。

---

## 已完成的批次里值得记的三件事

### P1 · `calamine 0.22 → 0.28`（唯一需要改代码的中风险包）

三处 API 变更（`core/src/knowledge/parser.rs`）：

1. `worksheet_range` 直接返回 `Result<Range<Data>, _>`（旧版是 `Option<Result<..>>`）；
2. `DataType` 变成 **trait**，枚举变体在 `Data` 上（旧写法 `DataType::String` 报
   `expected a type, found a trait`）；
3. `Data::Duration` 变体移除；`CellErrorType` 不再实现 `Display`（只剩 `Debug`）。

⚠️ **一处行为变化（已知并接受）**：xlsx 内**错误单元格**的文本从 Excel 显示式（如
`#DIV/0!`）变为枚举 Debug 名（如 `Div0`）。罕见路径（仅导入含错误公式的 Excel）。

### P2 · `pulldown-cmark 0.9 → 0.13`（跨 4 个大版本）

只因为它在我们这里只负责「扫 `Heading` 事件提取 TOC」，仅 2 处变更：
`Tag::Heading` 由 tuple → struct 变体；`Event::End` 携带 `TagEnd` 而非 `Tag`。
判别力由 `knowledge::parser::tests::test_markdown_parser` 锁住。

### P3 · `schemars 0.8 → 1.0`（原本是最高风险，实测零影响）

1.0 从 draft-07 换到 2020-12、`definitions` → `$defs`，本该改变 56 个工具的 schema。
实测 **`catalog-gen` 产物与现有 `tool-catalog.md` 逐行 diff = 0**：我们的工具参数结构体
几乎全是扁平类型（无嵌套结构体引用），两版走同一条最简路径。

---

## 方法论：本台账记录的一条教训

升级期间两次「失败」都不是真实回归：

1. **前端全量 vitest 报 17 个失败** → 单跑涉事文件 11/11 全绿，耗时数据异常
   （`setup 656s` vs 正常 ~2.7s）→ 真因是后台 `cargo test --workspace` 占满 CPU。
2. **`ServeDir::not_found_service` 看起来「版本不支持」** → 读源码发现它内部
   `SetStatus(NOT_FOUND)`，语义本来就是「404 + 自定义 body」，是我用错 API。

**结论：在资源争抢或信息不足时，不要急着归因于「版本/环境问题」——先做隔离实验或读源码。**

---

## F3′：为什么 `msw` 停在 2.x、`tailwindcss` 停在 3.4

### `msw 2.15 → 3.0`：暂缓

3.0 移除了 `listen({ onUnhandledRequest })`（`SharedOptions` 里已无此字段）。更关键的是
**它的异步管道与 `vi.useFakeTimers()` 组合后不再完成**：`KnowledgeTabs.test.tsx` 的 3 个
用例在 fake timers 下挂载请求悬挂 → 列表渲染为空 → 断言「找不到删除按钮」。

评估：msw 只是**测试工具**，我们没用到任何 3.0 新 API；而修这 3 个用例需要逆推新版内部
时序（`queueMicrotask` vs `setTimeout` 的推进时机），成本 > 收益。回退 2.15 后 506 个测试
立即全绿（实测）。

**重新评估触发条件**：msw 2.x 停止维护/出现安全问题，或某天必须用 3.0 的新特性。

### `tailwindcss 3.4 → 4.x`：暂缓

4.x 是**配置体系重写**（CSS-first：`@import "tailwindcss"` + `@theme` / `@custom-variant`，
PostCSS 插件换成 `@tailwindcss/postcss` 且内置 autoprefixer）。本项目的配置虽然简单
（`darkMode: 'class'` + 两个色值 + `fontSize` 覆盖 xs/sm），但迁移影响的是**所有组件的
视觉表现**——而当前**没有视觉回归测试**（vitest 只测行为，不测样式），改错只能靠人工看 UI
才能发现。

评估：3.4 仍在维护、无安全压力；我们未使用任何 4.x 新特性。

**重新评估触发条件**：有了视觉回归手段（如 Playwright 截图基线对比），或需要 4.x 的
新特性（容器查询内置、`@starting-style` 等）。
