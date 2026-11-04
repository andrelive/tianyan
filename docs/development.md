# 开发指南

天演开发者的构建、测试与贡献入口。**构建与验证命令的单一出处是本文件**（其他文档指过来，不复制命令）。

## 构建与检查

### Rust —— 主 workspace

```powershell
cargo check --workspace                     # 快速编译检查
cargo fmt --all -- --check                  # 格式检查
cargo clippy --workspace                    # Clippy
cargo test -p tianyan-core --lib            # core 单元测试
cargo test -p tianyan-server --lib          # server 单元测试
.\scripts\test.ps1 lint                     # fmt + clippy（含 tool-catalog 一致性门禁）
.\scripts\test.ps1 all                      # lint + unit + integration + e2e + bench
```

### `gui-native/` —— GPUI 原生客户端（独立 workspace，**主发布渠道**）

```powershell
cd gui-native; cargo build --release      # 独立 workspace，不进主 workspace
cd gui-native; cargo clippy               # 原生端 Clippy
pwsh -File scripts/verify-gui-native.ps1  # 自动化验收：一条命令跑完全部断言（复用或隐藏启动，不闪窗口）
```

- 出包（版本号落点 / 本地打 MSI / CI 发布链 / 已知坑 / 验收清单）→ [`operations/release-msi.md`](operations/release-msi.md)。
- 验收脚本**必须用 PowerShell 7+（`pwsh`）**：5.1 的 `Invoke-RestMethod` 在响应头没有 `charset` 时按 ISO-8859-1 解码 JSON——读到的中文本身是乱码，再写回服务端就会写坏数据。

### `gui-vite/` —— 已退役的 Web 前端（**不是产品前端，不随包发布**）

2026-11 退役：native（`gui-native/`）是唯一产品前端，本目录不再进 CI、
不再接新功能，仅作参考实现与手工调试保留（server 已不再托管其 `dist`，
浏览器访问请用 vite dev server 的代理）。

```powershell
npm run dev            # Vite dev server（端口 5100，/api 代理到 3000）
npm run test           # vitest 单元测试
npm run typecheck      # tsc --noEmit
npm run test:e2e       # Playwright e2e（后端须避开 3000，见 AGENTS.md）
```

文档地图（我要做什么该读哪份）→ [docs/README.md](README.md)。

## 知识固化判据（`.scratch/` 是临时区）

`.scratch/` 被 gitignore、**从不提交**——排查/实测得到的高价值结论若只留在那里，等于随会话丢失。

- **临时区**（可以放）：命令日志、一次性探针脚本、中间 JSON、审计草稿。
- **须固化到 `docs/`**（同一工作周期内）：机制说明（怎么运作）、契约（谁能改什么）、排查路径（故障现象 → 判定 → 处置）、实测数据（含测量条件与来源）。
- **固化位置**：机制 → `docs/architecture/`；运维/排查 → `docs/operations/`；决策 → `docs/architecture/decisions/`（ADR 正文不改，演进写"后续演进"）。
- 判据一句话：**"下次有人遇到同样问题，只看 docs/ 能否自己搞定？"** 能，就算固化完成。

## 重构纪律（多 agent / 并行工作）

- 禁止全局 git 操作（stash / reset --hard / checkout . / clean）——多 agent 并行工作会互相毁灭编辑
- 只允许 `git diff` / `git status` / `git checkout <具体文件>` / `git log` / `git reflog`
- 重构方法论（审查前置 / 波次 / QA 纪律）见下一章

## 重构实践

> 2026-08-13 架构深化（死代码清理 / 去重 / 错误语义化 / 组合根收敛）后沉淀。目标：后续任何 agent
> 在本仓库做架构审查与重构时**不重复犯已犯过的错**。配套规则：根 `AGENTS.md`「协作纪律」+ ADR-014。

### 1. 审查前置：先读决策记录，再提方向

开始任何架构审查前，**先读**：

1. [`architecture/decisions/REJECTED.md`](architecture/decisions/REJECTED.md) — 明确否决项 + **重新评估触发条件**。凡被否决的方向，除非触发条件满足，不要重新建议。
2. 相关 ADR（尤其 ADR-001 VFS / ADR-007 依赖环 / ADR-013 唤醒 / ADR-014 错误语义化）。
3. 根 `AGENTS.md`「设计原则」——seam 原则、不叠加抽象、共享基础设施归属被依赖方。

**教训来源**：某次审查初期提出的 `pipeline.run` 删除方向，实际是**有意的延后/保留**（文档注释已声明）。先读 REJECTED 和模块文档可避免浪费一轮探索。

> **文档漂移是常态**：文档可能声称某组件"已删除"而它仍是生产默认实现。**一切以代码为准**（`AGENTS.md` 开头规则），发现漂移顺手在文档加注或修正。

### 2. 波次执行法

```
探索（并行 explore agents + 自己读文档，不重叠）
  → plan agent 产出决策完备的任务图（波次 + 并行组 + 依赖 + 验证命令）
  → 每波并行执行 → 波末全量 gate（fmt + clippy -D warnings + core/server 测试）
  → 最终：全套件 + 独立审查（Oracle）+ 手工 QA
```

- **每波 gate 全绿才进下一波**——波内文件域不相交是并行前提。
- **同一波内不做相互依赖的变更**（例：executor 错误发射与 server 消费必须一次原子落地，否则中间态把 409 降级成 500）。
- **重名符号按符号全库 `rg`，不要按文件删**：`search_by_visual` 曾同时存在于 `DualLayerRetriever` 与 `VfsSearch` trait 上，只删了前者，trait 上的副本漏网直到复审才发现。
- 删除死代码的验证协议：`rg "<symbol>" --type rust`（含测试）→ 仅剩定义 → 编译通过。**编译器是删除操作的最终验证器**。
- 保留有意的死代码（如可选后端 `LocalFileBackend`，ADR-005 起生产默认是 `SqliteBackend`）时，在提交说明 / 文档注释中写明"有意的"。

### 3. 错误分类：只用语义谓词

见 ADR-014。一句话规则：**发射端用构造器，消费端用谓词，禁止字符串匹配**。

- 新增错误类别 → 改 `core/src/common/error.rs` 一处（KIND 常量 + 构造器 + 谓词 + 碰撞矩阵测试）。
- server 层映射 HTTP 状态码 → 用 `is_not_found` / `is_conflict` / `is_invalid_input` / `is_permission` / `is_timeout`。
- 曾犯的错：`contains("快照")` 把解析失败也判 404；`contains("块")` 把任意含"块"字错误判 409——**子串匹配过宽会静默改变 HTTP 语义**。

### 4. 组合根收敛

- **同进程共享组件只构造一次**：`PersistentSessionManager`、`TraceCollector` 等由 `AppState::new`（组合根）创建，注入 `AgentBuilder`；热重载（`reload_agent`）复用同一实例。
- 验证：`rg "PersistentSessionManager::new" server/src` 计数 ≤ 1。
- 曾犯的错：API 层与 Agent 各建一个 SessionManager 实例 → chat 服务必须"建会话后清空重写"（create-then-wipe），重启后双写混乱。

### 5. 手工 QA 纪律

- **副作用可回滚是前提**：`PUT /api/v1/config` 会持久化写入 `tianyan.toml`——QA 后必须恢复（git 不追踪该文件，手工改回）。
- 起服务器：`cargo run -p tianyan-server`（默认 3000），测完 `Stop-Process` + 确认端口释放。
- 端点矩阵至少覆盖：health / chat 端到端 / 错误分类（404/400/409/403/504 各一）/ 会话持久化（重启后仍在）/ 热更新 reload。
- 原生端界面行为验证走 ADR-052 自动化通道（`scripts/verify-gui-native.ps1`），**不要**靠"反复启动 + 人眼看 + 截图比对"。
- QA 临时文件（temp 目录、测试会话）**记入 TODO 并在交付前清理**。

## 运行

```powershell
cargo run -p tianyan-server   # 后端 127.0.0.1:3000
```

配置固定 `~/.tianyan/tianyan.toml`（ADR-023）；数据目录由配置 `storage.data_dir` 决定。
