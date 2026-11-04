# ADR-054：Web 端（`gui-vite`）退役

- **状态**：已采纳（阶段 1 已实施 2026-11；阶段 2 待执行）
- **日期**：2026-11-04
- **相关**：ADR-049（插件化边界与装配）、ADR-051（桌面单 server 复用）、ADR-052（GUI 自动化端口）

## 背景

天演在 1.x 期间同时维护两个前端：`gui-vite/`（React + TypeScript，Web）与
`gui-native/`（GPUI，原生）。原计划是「原生壳做轻量入口、Web 端做完整界面」，
但实际情况早已倒转：

1. **生产链从不包含 Web 端**。`release.yml` 的 MSI 只装 `tianyan-gui-native.exe`
   （纯 Rust 前端），头注释原文：「不构建 gui-vite：native 是纯 Rust 前端，MSI 只装 exe」。
   终端用户手里的产品从来没有 Web 界面。
2. **功能对齐已经完成**。native 覆盖了会话/消息流式/工具卡片/知识/记忆/技能/角色/
   定时任务/审批/工作区/设置（含 14 栏）与配置向导，设置页模型配置等处比 Web 版更强
   （用户原话：「vite 版本的模型设置其实不大好使」）。
3. **它仍以五处硬耦合牵制 Rust 测试与 CI**，其中最硬的一处不是「多跑一点」，而是
   「能不出包」：

| # | 耦合点 | 后果 |
|---|---|---|
| 1 | `server/tests/e2e_tests.rs::test_e2e_spa_subroute_serves_index_html` **硬断言** `gui-vite/dist/index.html` 存在（缺产物 panic，不是 skip） | CI 必须把 `npm ci && npm run build` 排在集成测试**之前**；缺产物 → quality 红 → `release.build`（`needs: quality`）被跳过 → **tag 推了不出包**（v1.0.0 / v1.0.1 实发事故） |
| 2 | `quality.yml`：`setup-node` + npm 缓存 + build/lint/typecheck/vitest 四步 | CI 多一个 node job |
| 3 | `scripts/test.ps1`：lint / unit 内的前端步骤 + `gui-build` / `gui-e2e` 两个层级 | 本地门禁三层依赖 node |
| 4 | `server/src/lib.rs`：`get_static_dir`（猜 6 条路径，其中 3 条是已删 `gui/dist` 的遗留）+ SPA 回退 handler + `TIANYAN_STATIC_DIR` | `tower-http` 的 `fs` feature 只为它存在 |
| 5 | `scripts/capture-contract-fixtures.ps1` + `scripts/e2e/tianyan.e2e.toml` | 只服务 Web 端测试 fixture 与 playwright 后端配置 |

关键判断：**「放着不管」不是零成本**。第 1 条使任何 Rust 测试运行都要先有一份 JS 产物，
而这份产物只在开发者本地因为「跑过一次 npm build」而恰好存在——这是典型的
「本地绿 / CI 红」陷阱，且已经在两个版本上真实烧过。

## 决策

按两阶段退役，把「解除耦合」（可逆、纯减重）与「物理删除」（不可逆）分开：

### 阶段 1（已实施，2026-11）

- `server`：删除静态资源服务与 SPA 回退（`get_static_dir`、`/assets` 的 `ServeDir`、
  `TIANYAN_STATIC_DIR`、`tower-http` 的 `fs` feature）。服务只提供 `/api/v1/**` 与
  `/health`；未知路径由 axum 默认 404 处理（`/api/v1/**` 的未知路径仍由
  `api_not_found` 返回 404 **JSON**，该用例保留）。
- 删除 `test_e2e_spa_subroute_serves_index_html`。
- CI（`quality.yml`）：移除 `setup-node` + npm 缓存 + 前端 build / lint / typecheck / vitest。
- 本地门禁（`scripts/test.ps1`）：移除 `gui-build` / `gui-e2e` 层级与 lint / unit 里的前端步骤。
- 文档：`AGENTS.md`、`README.md`、`docs/development.md`、`docs/operations/ci-pipeline.md`、
  `docs/architecture/module-map.md`、`docs/known-limits.md` 标注退役状态。
- `gui-vite/` 目录**原地保留**（`npm run dev` 手工仍可用——vite dev server 自己起 5100
  并经 proxy 打后端 3000，不依赖 server 的静态服务）。

### 阶段 2（待执行）

目录物理删除 + 文档中 `gui-vite` 相关描述全量清理。触发条件：native 这轮功能复验完成、
且确认无遗漏交互（建议按「vite 可见项 → native 状态」三态表过一遍，已知待核对项：
`components/clipboard/ClipboardConfirmBar.tsx` 在前端是否有 native 对应物）。

## 代价（诚实记账）

- **丢失浏览器端到端覆盖**：`gui-vite/e2e/` 的 12 个 Playwright spec（含 4 个
  `real-*.spec.ts`：真实后端 + 真实会话/知识库/工作区）。`server/tests/e2e_tests.rs`
  覆盖 HTTP/SSE 协议本身，但**浏览器实现层**（EventSource 重连、DOM 流式渲染、
  React 状态机）只有它们覆盖。native 的等价物是 ADR-052 的进程内
  `GET /state` + `POST /act`，验的是 GPUI 界面，两者不同层，不能互替。
- **少一个调试面**：浏览器开两个 tab 观察同一后端的流式与 per-session 隔离。
- **`npm run build` 产物不再有托管方**：只能 `npm run dev` 手工开发。

## 不做的事

- 不为 Web 端保留一条「可选装配」路径（例如按 feature 装静态服务）。理由：那会让
  「有没有 Web」变成运行期开关，而 ADR-049 的教训是**可选装配必须显式降级并告警**，
  这里没有需要它的场景——需要 Web 界面时 `npm run dev` 即可。
- 不把 `gui-vite` 迁到 `docs/archive/`。它体量 1.35 MB / 185 个 ts(x) 文件，
  归档目录不该承载可运行代码；历史记录由 git 承担，阶段 2 直接删除。
