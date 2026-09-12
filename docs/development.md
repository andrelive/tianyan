# 开发指南

天演开发者的构建、测试与贡献入口。

## 构建与检查

```powershell
cargo check --workspace                     # 快速编译检查
cargo fmt --all -- --check                  # 格式检查
cargo clippy --workspace                    # Clippy
cargo test -p tianyan-core --lib            # core 单元测试（1181 个）
cargo test -p tianyan-server --lib          # server 单元测试
.\scripts\test.ps1 lint                     # fmt + clippy（含 tool-catalog 一致性门禁）
.\scripts\test.ps1 all                      # lint + unit + integration + e2e + bench
```

前端（`gui-vite/`）：

```powershell
npm run dev            # Vite dev server（端口 5100，/api 代理到 3000）
npm run test           # vitest 单元测试（392 个）
npm run typecheck      # tsc --noEmit
npm run test:e2e       # Playwright e2e
```

## 文档约定

| 文档 | 定位 |
|------|------|
| [`docs/architecture/decisions/`](architecture/decisions/) | 架构决策记录（ADR-001~033）——**历史记录，不修改正文**；演进在末尾加"后续演进" |
| [`docs/architecture/module-map.md`](architecture/module-map.md) | 模块索引（职责/位置/关键文件）——保持最新 |
| [`docs/module-descriptions.md`](module-descriptions.md) | 模块详细说明——保持最新 |
| [`docs/module-relationships.md`](module-relationships.md) | 模块间关系——保持最新 |
| [`docs/architecture/tool-catalog.md`](architecture/tool-catalog.md) | **自动生成**（`scripts/gen-tool-catalog.ps1 -Check` 门禁）——改代码 schema，不改此文件 |
| [`docs/architecture/context-pipeline.md`](architecture/context-pipeline.md) | 上下文组装顺序 + 压缩机制 + 构成量化方法 + 故障模式（机制级，先读它） |
| [`docs/architecture/event-protocol.md`](architecture/event-protocol.md) | 事件通道模型 + 4 类事件字段表 + 订阅/快照恢复 + 可靠性分层 |
| [`docs/architecture/model-provider-notes.md`](architecture/model-provider-notes.md) | Provider 矩阵 + reasoning 回传契约 + 思考强度×语言实测 + 前缀缓存 + 上游异常 |
| [`docs/operations/`](operations/) | 运维手册（troubleshooting / data-health-check / release-msi） |
| [`docs/archive/`](archive/) | 历史存档（已完成/被取代的文档）——不再作为事实来源 |

**代码是唯一事实来源**：文档与代码冲突时以代码为准（根目录 `AGENTS.md` 的完整规范）。

### 知识固化判据（`.scratch/` 是临时区）

`.scratch/` 被 gitignore、**从不提交**——排查/实测得到的高价值结论若只留在那里，等于随会话丢失。

- **临时区**（可以放）：命令日志、一次性探针脚本、中间 JSON、审计草稿。
- **须固化到 `docs/`**（同一工作周期内）：机制说明（怎么运作）、契约（谁能改什么）、排查路径（故障现象 → 判定 → 处置）、实测数据（含测量条件与来源）。
- **固化位置**：机制 → `docs/architecture/`；运维/排查 → `docs/operations/`；决策 → `docs/architecture/decisions/`（ADR 正文不改，演进写"后续演进"）。
- 判据一句话：**"下次有人遇到同样问题，只看 docs/ 能否自己搞定？"** 能，就算固化完成。

## 重构纪律

- 禁止全局 git 操作（stash / reset --hard / checkout . / clean）——多 agent 并行工作会互相毁灭编辑
- 只允许 `git diff` / `git status` / `git checkout <具体文件>` / `git log` / `git reflog`
- 重构实践（审查/波次/QA 纪律）见 [`docs/architecture/refactoring-practices.md`](architecture/refactoring-practices.md)

## 运行

```powershell
cargo run -p tianyan-server   # 后端 127.0.0.1:3000
npm run dev                   # 前端 Dev（gui-vite/，5100）
```

配置固定 `~/.tianyan/tianyan.toml`（ADR-023）；数据目录由配置 `storage.data_dir` 决定。
