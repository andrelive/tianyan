# 开发指南

天演开发者的构建、测试与贡献入口。

## 构建与检查

```powershell
cargo check --workspace                     # 快速编译检查
cargo fmt --all -- --check                  # 格式检查
cargo clippy --workspace                    # Clippy
cargo test -p tianyan-core --lib            # core 单元测试（1199 个）
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
| [`docs/archive/`](archive/) | 历史存档（已完成/被取代的文档）——不再作为事实来源 |

**代码是唯一事实来源**：文档与代码冲突时以代码为准（根目录 `AGENTS.md` 的完整规范）。

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
