# 编程助手能力差距评估与选型

> 状态：**已实施**（2026-08-07，D1-D7 决策点均已实现；前端工作台为 Phase 1 只读，Phase 2 编辑按 D5 渐进式设计留待后续；§4 选型研究保留供追溯）
> 日期：2026-08-06（实施记录见 [§6](#6-实施记录)）
> 范围：天演作为编程助手（coding assistant）的能力差距评估，以及各差距项的技术选型调研

## 1. 背景与目标

天演目前是一个通用本地智能代理系统（聊天 + 知识库 + 记忆 + 技能 + 定时任务）。本文档评估其承担**编程助手**工作时的能力差距，并为每项差距检索业界最佳方案，供选型讨论后形成实施决策。

评估基线为 `docs/system-architecture.md` 与代码事实（本文档所有结论均以代码为准）。

## 2. 现状盘点（已有能力）

### 2.1 编程相关工具（6 个直接工具 + 6 个技能）

注册点：`core/src/agent/tool_registry/mod.rs:391-490`，共 14 个内置工具。

| 工具 | 功能 | 边界 |
|------|------|------|
| `read_file` | 读取文件全文 | 无行号/offset/limit，无分段读 |
| `write_file` | 整文件覆盖写入 | 无 patch/部分编辑，参数仅 `{path, content}` |
| `execute_command` | 执行 shell 命令（30s 默认超时） | strict 模式禁 `&&`/`;`/解释器；git 等中危命令需审批 |
| `search_code` | ripgrep 搜索 | 无 include/exclude/上下文行/正则 flag，每文件限 20 条 |
| `run_tests` | 运行测试命令并解析 | 需 LLM 自拼命令，无测试发现 |
| `verify_build` | 构建验证（LLM-as-Judge 门控） | 退出码 + 错误模式匹配 |

经 `call_skill` 暴露的技能：file_read / file_write / file_delete / file_list / system_command / http_request（独立实现，非工具同一路径）。

### 2.2 变更管理与回退（最接近 diff 的现有资产）

`core/src/snapshot/mod.rs`：

- **存储**：内容寻址对象库（`objects/{sha256前2位}/{sha256}.bin`）+ 树文件（`trees/{index}.json`，path→sha256）+ mtime/size 缓存
- **触发**：每条用户消息处理前 capture（`coordinator.rs:172/217`），索引 = 消息索引
- **回退/重做**：`restore()` 整树恢复（含删除新增文件）；`save_redo`/`load_redo` 支持撤销回退
- **接线**：`POST /api/v1/sessions/{id}/messages/delete`（回退）与 `messages/redo`（重做），`server/src/api/sessions/services.rs:164/237`
- **边界**：消息粒度（非操作粒度）；需配置 `working_directory`；排除 `.git`/`node_modules`/`target` + 2MB 单文件上限；**不产生 diff 输出**

### 2.3 可扩展通道

- **MCP 客户端**（`mcp/`，stdio，tools-only）：已通过 `server/src/mcp_bridge.rs` 把外部 MCP 工具注册进 Agent 工具循环 —— codegraph/playwright 等 stdio MCP 服务器可直接配置接入
- **审批/安全门控**（`executor/`）：命令黑名单、路径沙箱、rm→回收站、风险分级人工审批、审计

### 2.4 前端与 API

- API：40+ 业务端点，覆盖 chat/sessions/knowledge/skills/config/insights 六域（`server/src/api/`）
- 前端：纯聊天 UI + 管理面板（`gui-vite/src/`），无编程工作台组件
- 桌面：Tauri 2 包装，无自定义 command

## 3. 差距清单

### P0 — 编程助手的可靠性基础（不做则写代码不可靠）

| # | 差距 | 现状证据 | 影响 |
|---|------|----------|------|
| G1 | **语义化编辑（apply_edit）** | `write_file` 仅全量覆盖（`tool_params.rs:14`） | 大文件必须整文件重写，Token 爆炸且易出错 |
| G2 | **Diff 生成/解析/应用** | 全 workspace 无 diff 相关代码与依赖 | 无法展示变更、无法精确应用修改 |
| G3 | **文件系统浏览** | 无 list_files/glob 工具；snapshot 内部有 walk 但未暴露（`snapshot/mod.rs:333`） | Agent 无法获知工作区结构 |

### P1 — 体验提升

| # | 差距 | 现状证据 |
|---|------|----------|
| G4 | `read_file` 行号/分段读取 | `actions.rs:12` 全文 read_to_string |
| G5 | `search_code` 高级参数 | `actions.rs:28` rg 基础包装 |
| G6 | **Git 集成** | 无 `git2` crate、无 git 调用；仅审批名单含 "git" 字符串（`workflow.rs:108`） |

### P2 — 差异化能力

| # | 差距 | 说明 |
|---|------|------|
| G7 | LSP 集成（诊断/符号/跳转/引用/重命名） | 完全缺失；`verify_build` 无编译错误定位 |
| G8 | 测试发现与选择 | `run_tests` 需 LLM 自拼命令 |
| G9 | 前端编程工作台（diff 面板/文件树/编辑器） | gui-vite 无相关组件 |
| G10 | MCP 客户端资源支持 | tools-only，无 resources/prompts |

## 4. 选型研究（2026-08 调研）

> 研究方法：下载 Claude Code / opencode / Continue / Aider / Codex 源码逐一精读 + 官方文档 + 社区逆向交叉验证。所有结论标注来源。

### 4.1 语义化编辑方案（G1）

**业界两条原语路线**：

| 路线 | 代表 | 原语 | 组织粒度 |
|------|------|------|----------|
| old_string/new_string 精确替换 | Claude Code、opencode `edit`、Continue `multiEdit` | `{file_path, old_string, new_string, replace_all?}` | 单文件 |
| unified diff freeform | Aider `udiff`、Codex `apply_patch`、opencode `apply_patch` | `*** Update File` + `@@` hunk 信封 | 多文件 + move/delete |

**模糊匹配技术谱系**（按激进程度）：

1. 归一化级联（Claude/Codex/opencode-patch 共识）：exact → 引号/Unicode 归一 → 空白归一 → tab/space；命中后坐标回射原文
2. 多级 Replacer（opencode edit 的 9 级级联，含首尾锚点 + Levenshtein 0.65 阈值）
3. diff-match-patch 模糊 patch（Aider，Match_Threshold 参数）
4. git 3-way cherry-pick 解冲突（Aider `flexible_search_and_replace`）
5. **缩进重投影**（Continue 独有）：fuzzy 命中后 newString 缩进重算到实际匹配位置

**关键共识**：
- **失败必须显式**：错误文本回喂模型（Aider `reflected_message`、Codex `RespondToModel`），指导模型"提供更多上下文"或 `replace_all`
- **不做无锚点的纯 fuzzy**：Aider 的 edit-distance fuzzy 已在源码中禁用（`return` 短路），Continue 的 Jaro-Winkler 也因坐标映射 bug 被注释——高相似度匹配容易改错位置
- **应用前全量验证**（Codex `verify_apply_patch_args`）：解析 → 读文件 → 计算 → 确认差异非空 → 才写盘
- **编辑后验证闭环**：opencode 编辑后强制 LSP diagnostics 注入工具输出（"LSP errors detected in this file, please fix"）
- 批量原子性：Continue `multiEdit`（同文件多编辑一次调用、失败全回滚）+ Codex patch 信封（多文件）

**推荐（候选方案 A，待讨论）**：
1. 双原语并存：`apply_edit`（old/new 精确替换，小改）+ `apply_patch`（unified diff 信封，大改/多文件）
2. 匹配级联：归一化（引号/Unicode/空白）→ 前导空白容忍 → 首尾锚点 + 行相似度阈值 → 失败即报错；**不做纯 fuzzy**
3. 命中后坐标回射 + 缩进重投影（Continue 模式）
4. 应用前全量验证 + 错误回喂模型
5. 编辑后验证：天演无 LSP 时先用 `cargo check` 或强制重读校验（对应 G7 阶段一）

**来源**：Claude Code [tools-reference](https://code.claude.com/docs/en/tools-reference) + [逆向实现](https://github.com/claude-code-best/claude-code/blob/main/packages/builtin-tools/src/tools/FileEditTool/utils.ts) + [finisky 逆向](https://finisky.github.io/en/claude-code-edit-tool/)；opencode [edit.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/edit.ts)（9 级 Replacer + 防呆）+ [patch/index.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/patch/index.ts)（seekSequence）；Aider [editblock_coder.py](https://github.com/paul-gauthier/aider/blob/main/aider/coders/editblock_coder.py) + [search_replace.py](https://github.com/paul-gauthier/aider/blob/main/aider/coders/search_replace.py) + [udiff_coder.py](https://github.com/paul-gauthier/aider/blob/main/aider/coders/udiff_coder.py)；Continue [multiEdit.ts](https://github.com/continuedev/continue/blob/main/core/tools/definitions/multiEdit.ts) + [findSearchMatch.ts](https://github.com/continuedev/continue/blob/main/core/edit/searchAndReplace/findSearchMatch.ts) + [performReplace.ts](https://github.com/continuedev/continue/blob/main/core/edit/searchAndReplace/performReplace.ts)；Codex [parser.rs](https://github.com/openai/codex/blob/main/codex-rs/apply-patch/src/parser.rs) + [seek_sequence.rs](https://github.com/openai/codex/blob/main/codex-rs/apply-patch/src/seek_sequence.rs) + [invocation.rs](https://github.com/openai/codex/blob/main/codex-rs/apply-patch/src/invocation.rs)

### 4.2 Diff 库选型（G2）

**结论：引入 `similar`（3.1.2，2026-08-04），patch 应用自研薄层。**

对比：

| Crate | 算法 | unified 生成 | 解析 | 应用 | 评价 |
|-------|------|-------------|------|------|------|
| **similar** | Myers/Patience/Histogram/Hunt/LCS + deadline | ✅ 成熟（3.0 修复 range bug） | ❌ | ❌ | **推荐**：零依赖、词级高亮（`iter_inline_changes`）、insta 同源 |
| diffy 0.5.1 | Myers | ✅ | ✅ PatchSet（多文件 git diff） | ✅ GNU patch 式严格匹配（无 fuzz） | 唯一全家桶，但对 LLM 生成的 patch 容错不足 |
| imara-diff | Myers/Histogram（最快） | ⚠️ | ❌ | ❌ | helix/delta 在用，可作算法参考 |
| gix-diff | 委托 gix-imara-diff | ⚠️ 面向 git 对象 | ❌ | ❌（gitoxide 无 apply） | 过度工程 |
| git2 (libgit2) | C 移植 | ✅ | ✅ | ✅ git_apply（最专业） | 重量级依赖，与 G6 结论冲突，排除 |

**生态关键发现**：Codex（Rust）、opencode、continue、Aider **全部自研 patch 应用**（codex `parse_patch` + `seek_sequence` 模糊定位，~300 行），因为 GNU patch 式严格匹配对模型输出失败率不可接受。unified diff 只用于展示。

**推荐**：
1. 工具展示：`similar` 输出 unified diff（context 3 行）
2. apply_edit 内部：自研 `parse_patch` + 模糊定位（参考 codex 模式），验证用 sha256 与 snapshot 对象库衔接
3. 前端数据源：`similar` 结构化行序列（tag + 词级高亮区间）序列化 JSON

**来源**：[similar](https://crates.io/crates/similar) / [diffy](https://crates.io/crates/diffy) / [codex apply-patch 源码](https://github.com/openai/codex/blob/main/codex-rs/apply-patch/src/lib.rs) / [opencode patch](https://github.com/sst/opencode/blob/dev/packages/opencode/src/patch/index.ts) / [gitoxide issue #301](https://github.com/Byron/gitoxide/issues/301)

### 4.3 文件浏览工具设计（G3）

**业界趋势**：opencode 2026 年已删除独立 list 工具（目录列举并入 read），glob 与 list 职责分化；Claude Code 引导"列目录用 shell `ls`"。

**推荐（候选方案 B）**：
- `glob(pattern, path?)`：按名称模式找文件，返回绝对路径、按 mtime 排序、硬上限 100-200 + truncated 标志
- `list_dir(path, offset?, limit?, ignore?)`：列单层目录，目录加 `/` 后缀，分页
- read_file 增加目录模式（opencode 式）：目录返回条目列表，天然替代 list 工具
- 两者尊重 `.gitignore`（rg --files 天然行为）

**来源**：[opencode read.ts（目录模式）](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/read.ts) / [glob.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/glob.ts) / Claude Code [sdk-tools.d.ts](https://cdn.jsdelivr.net/npm/@anthropic-ai/claude-code@2.0.76/sdk-tools.d.ts)

### 4.4 读取与搜索工具增强（G4/G5）

**read_file 增强**（融合 opencode + cline，规避 Claude Code 静默截断缺陷）：
- 参数：`file_path`(必), `offset`(1-indexed), `limit`(默认 2000)
- 返回：行号前缀 `N: line` + **显式截断消息** `(Showing lines X-Y of Z. Use offset=N to continue.)` + 结构化 `truncated` 标志（Claude Code 静默截断导致假阴性报告是社区最痛教训）
- 行为：offset 越界报可行动错误、文件不存在给相近文件名建议、二进制嗅探（NUL/不可打印 >30%）、单行 2000 字符截断、重复读取检测（cline mtime + 3 次）

**search_code 增强**（以 Claude Code 参数面为蓝本）：
- 参数：`pattern`(必), `path`, `glob`, `output_mode`(默认 files_with_matches 省 token), `type`, `-n/-i/-C/-A/-B`, `head_limit`(默认 200), `offset`, `multiline`
- 内部防护（opencode 实测数字）：单条记录 64KB 拒绝、submatch 100 上限、匹配行 2000 字符截断、`.git` 排除、无效正则检测（exit 2 + `regex parse error`）
- "无结果"与"分页耗尽"区分（Claude Code `No entries at this offset`）

**统一截断层（横切所有工具）**：maxLines=2000 + maxBytes=50KB 双上限；read/grep 保头（head）、命令输出保尾（tail，错误在末尾）；截断后全文落盘 + 路径提示 + 委托指令；所有工具显式 `truncated` 标志（opencode 在 tool 包装器统一应用）

**来源**：[opencode truncate.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/truncate.ts) / [shell.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/shell.ts) / [ripgrep.ts](https://github.com/sst/opencode/blob/dev/packages/core/src/ripgrep.ts) / [cline read_file](https://github.com/cline/cline/blob/main/src/core/prompts/system-prompt/tools/read_file.ts)

### 4.5 Git 集成与 checkpoint 机制（G6）

**业界共识（2026）**：用 git 对象层做轻量 checkpoint，**绝不自动 commit、不碰 refs**。

| 工具 | Checkpoint 载体 | undo | 是否自动 commit |
|------|----------------|------|----------------|
| opencode | git tree 对象（`git write-tree` → TreeID） | `/undo` git restore | ❌ |
| Codex | ghost commit（`git commit-tree` 无引用 commit） | `git restore --source` | ❌ |
| Claude Code | 自研文件快照（非 git，100 个/30 天） | `/rewind` | ❌ |
| Aider | 真实 git commit（每轮编辑后） | `git reset HEAD^` | ✅（历史遗留） |

**git2 vs CLI 生态风向**：cargo 正讨论从 libgit2 迁移 CLI（[cargo#17227](https://github.com/rust-lang/cargo/issues/17227)）、turborepo 已移除 git2（libgit2-sys 25s C 编译，[PR #12015](https://github.com/vercel/turborepo/pull/12015)）、Codex 明确注释不用 git2。**结论：不引入 git2，用 `Command::new("git")` + `GIT_OPTIONAL_LOCKS=0` + kill_on_drop + 超时。**

**推荐（候选方案 C）**：两者结合，git 优先：
1. git 仓库内：现有自研 snapshot 改为 opencode 式 tree 快照（`git write-tree` 拿 TreeID 作消息级快照，undo 用 `git restore --source`，不带 --staged 保留用户 index）——删除自研 sha256 对象库，少维护一套内容寻址系统
2. 非 git 仓库：保留现有 SnapshotManager 作 fallback（Claude Code 式定位）
3. 不做 Aider 式自动 commit；git 能力（status/diff/commit）暴露为工具
4. 风险：tree 快照不覆盖 gitignored 文件（需 force-include 或对这部分保留自研）

**来源**：[opencode git.ts](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/git.ts) / Codex [PR #3914](https://github.com/openai/codex/pull/3914) / [info.rs](https://github.com/openai/codex/blob/main/codex-rs/git-utils/src/info.rs) / Claude Code [checkpointing](https://code.claude.com/docs/en/checkpointing) / Aider [git.html](https://aider.chat/docs/git.html)

### 4.6 LSP 集成方案（G7）

**Rust 生态现状**：无生产级现成客户端 crate（lsp-client 实验性、async-lsp-client 偏可用）；Helix `helix-lsp` 与 Zed `crates/lsp` 都是 lsp-types + 自研传输/生命周期层（~500 行），模式清晰可抄。Rust 侧唯一成熟的"现成 LSP 客户端"是 **mcpls**（Rust 写的 MCP→LSP 桥）。

**工具暴露（行业标准已收敛）**：单工具 `lsp` + 9 operation（goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation / callHierarchy 等），参数 `{operation, filePath, line, character}`，执行前 `touchFile` 同步内容。诊断是**被动增量推送**（Claude Code 现行模式，编辑后自动附加），不是工具操作（opencode #16569 提案中）。

**推荐（候选方案 D，三阶段）**：
1. **阶段一（零新进程）**：`verify_build` 增加 `--message-format=json-render-diagnostics` 解析（`cargo_metadata::Message::parse_stream`），正则匹配升级为结构化诊断（file/line/col/level/code/suggestion）；`VerificationResult` 增加 `structured_diagnostics`；同时用 `tree-sitter-rust` 生成单文件符号大纲（喂 VFS 结构层）
2. **阶段二（核心）**：自研轻量 LSP 客户端，**只内置 rust-analyzer**（避免多语言矩阵），架构抄 opencode（服务器注册表 + 按文件路由 client 池 + push/pull 双通道诊断）；注意 rust-analyzer 诊断依赖落盘 cargo check（flycheck），虚拟内容拿不到完整诊断
3. **阶段三（可选）**：MCP 桥 mcpls 作为不维护客户端的备选；**不自研客户端 + MCP 壳两层叠**（违反 AGENTS.md"不叠加抽象"）
4. 能力优先级：诊断 > 定义跳转/hover > documentSymbol/workspaceSymbol > findReferences > rename（最后）

**来源**：[opencode lsp/](https://github.com/sst/opencode/blob/dev/packages/opencode/src/lsp/index.ts) / Claude Code [LSP 插件](https://code.claude.com/docs/en/discover-plugins) / [helix-lsp](https://github.com/helix-editor/helix/blob/master/helix-lsp/src/lib.rs) / [Zed lsp_store.rs](https://github.com/zed-industries/zed/blob/main/crates/project/src/lsp_store.rs) / [mcpls](https://github.com/bug-ops/mcpls) / [rustc JSON 规范](https://doc.rust-lang.org/rustc/json.html)

### 4.7 测试与构建验证增强（G8）

**业界现状**：**没有任何主流工具提供测试发现工具**——Claude Code/opencode/Codex/Cline 全无；测试命令来源是仓库 AGENTS.md 约定（Codex 自身明文"用 `just test`，别直接 `cargo test`"）或模型自拼。这是天演的差异化空白。

**推荐（候选方案 E）**：
1. 新增 `discover_tests`：Cargo.toml → `cargo test -- --list` / pytest `--collect-only -q` / package.json scripts，返回结构化 `[{suite, name, file, line}]`（上限 500）
2. `run_tests` 改造：`{filter?, suite?, framework?}`，失败重跑窄化由模型决策（工具不自作主张）
3. 结果解析：失败用例列表（name/file/line，≤20）+ 断言消息 + 回溯头 30 行/尾 20 行 + 统计；失败按文件分组方便批量修复
4. `verify_build`：见 4.6 阶段一（结构化诊断）+ 保留 LlmJudge 兜底（JSON 为空但退出码非 0 时语义判断）

**来源**：Codex [AGENTS.md](https://github.com/openai/codex/blob/main/AGENTS.md) / opencode 工具目录（无 test.ts）/ Claude Code [tools](https://code.claude.com/docs/en/tools)

### 4.8 前端工作台技术栈（G9）

**竞品实际选型**：opencode web 用 shiki + 自研 worker 渲染（@pierre/diffs）；Continue GUI 用 jsdiff + react-syntax-highlighter（无编辑器库）；Cline diff 走 VS Code 原生；Claude Code webview 用 Monaco 但遭遇 worker/CSP 崩溃事故。**聊天型 AI 工具全部避开在 webview 嵌完整编辑器**。

**Monaco vs CodeMirror 6**：

| 维度 | Monaco | CodeMirror 6 |
|------|--------|--------------|
| 体积 | 2-5MB gzipped（Replit 实测 5.01MB） | 50-200KB gzipped（evcc 实测 932KB→101KB） |
| Tauri/WebView2 | worker 创建失败事故（tauri#9595）+ CSP `worker-src blob:` 配置 + WKWebView LSP 包装层白屏 | 零 worker、零 CSP 特殊项 |
| diff | 内置 DiffEditor（VS Code 同款） | 官方 @codemirror/merge（split/unified、collapseUnchanged、超时保护） |
| LSP 前景 | monaco-languageclient 成熟但重 | 社区方案无第一方 |
| 移动端/多实例 | 不支持/成本高 | 友好 |

迁移证据：Replit（51MB→8.23MB）、Sourcegraph（JS -43%）、evcc、mdBin 全部 Monaco→CM6。

**推荐（候选方案 F）**：
- 编辑器/查看器：**CodeMirror 6**（只读一等公民，readOnly + editable 切换）
- diff：@codemirror/merge（同栈）；若避开编辑器则 react-diff-viewer-continued（虚拟化 + worker 计算）
- 文件树：react-arborist（虚拟化 + 内建 DnD/键盘导航，专为文件树设计）
- 高亮：react-markdown 保留；react-syntax-highlighter 先留后迁（聊天代码块评估迁 shiki，token 可复用于自渲染 diff，删后省 ~150KB）
- 渐进式：Phase 1 只读工作台（文件树 + CM6 只读 + merge 只读 diff）→ Phase 2 编辑（dirty 状态 + 保存）→ Phase 3 可选 Monaco/LSP

**来源**：[@codemirror/merge](https://github.com/codemirror/merge) / [react-arborist](https://github.com/jameskerr/react-arborist) / [Replit 迁移](https://replit.com/blog/codemirror) / [Sourcegraph 迁移](https://sourcegraph.com/blog/migrating-monaco-codemirror) / [tauri#9595](https://github.com/tauri-apps/tauri/discussions/9595) / [Continue gui package.json](https://github.com/continuedev/continue/blob/main/gui/package.json)

## 5. 待讨论决策点（带研究推荐）

- [x] D1：**已定稿** —— 语义化编辑双原语：`apply_edit` 采用 **hashline 机制**（行号+内容哈希锚点 `N#ID`，空白不敏感哈希、防陈旧校验、edits[] 批量 bottom-up 应用、错误回喂最新锚点）+ `apply_patch`（unified diff 信封，大改/多文件）。参考 oh-my-opencode（code-yeongyu，移植自 oh-my-pi）
- [x] D2：**已定稿** —— diff 库 `similar`（3.1.2，零依赖）+ 自研 apply 薄层（parse_patch + 模糊定位，参考 codex ~300 行）；unified diff 给模型展示，结构化行序列给前端
- [x] D3：**已定稿** —— 快照纯自研升级：保留现有 sha256 对象库 + trees 树 + mtime/size 缓存（ADR-006），新增 ① flate2/zstd 压缩 ② GC（树文件为可达集的标记-清除）③ 与 similar 集成产出 diff。**不引入 git 二进制 / git2 / gix**（否决影子仓库：需 git 二进制假设，与"全能型通用代理"定位冲突；否决 gix：restore 编排未完成；否决 git2：C 编译 + 生态逆风）。非 git 仓库天然支持
- [x] D4：**已定稿** —— 代码智能层三阶段全自研：① `verify_build` 多格式结构化诊断（格式注册表，按项目探测：Cargo.toml→cargo json、tsconfig→tsc parseable、pyproject→pytest json；`cargo_metadata::Message::parse_stream`）② 自研**多语言 LSP 注册表**（opencode 式：`{language_id, extensions, root探测(lockfile), spawn, 自动安装命令}` + 按项目启用 + 安装失败 broken 降级；工具暴露 `lsp` + 9 operation；起步内置 rust-analyzer/TS/pyright/gopls，机制预留扩展；lsp-types 纯 Rust 依赖）③ tree-sitter 多语言符号大纲喂 VFS 结构层（天然 500+ grammar）。能力优先级：诊断 > 定义跳转/hover > 符号 > findReferences > rename（最后）。LSP 诊断 = 内环快信号，verify_build = 最终权威门控
- [x] D5：**已定稿** —— 前端工作台：**CodeMirror 6**（只读一等公民）+ @codemirror/merge（diff，同栈）+ react-arborist（文件树），从零建 `/workspace` 页面；聊天侧 react-markdown + react-syntax-highlighter 不动；渐进式：Phase 1 只读工作台 → Phase 2 编辑（配合 apply_edit 展示）→ Phase 3 可选 Monaco 懒加载。否决 Monaco：2-5MB、Tauri worker 事故、竞品全部规避
- [x] D6：**已定稿** —— 新增 `discover_tests`（业界空白差异化点）：探测链 Cargo.toml→`cargo test -- --list`、pyproject→`pytest --collect-only -q`、package.json→`vitest --list`，返回结构化 `[{suite, name, file, line}]`（上限 500）；`run_tests` 改造 `{filter?, suite?, framework?}`；结果解析：失败用例 ≤20（断言消息 + 回溯头 30/尾 20 行）+ 统计，按文件分组；工具不做自主重试（决策权在 LLM）；与 D4 ①共享格式注册表模式
- [x] D7：**已定稿** —— 分期：**P0**（一次 PR 组，共用 similar + 工具链路）：similar 引入 → read_file 增强（行号+哈希锚点，apply_edit 前置）→ apply_edit(hashline) + apply_patch → 快照升级（压缩+GC+diff）→ 文件浏览（glob/list_dir）；**P1**：search_code 增强 + 统一截断层 → verify_build 多格式结构化诊断 → discover_tests + run_tests 改造 → tree-sitter 符号大纲；**P2**：后端 workspace API → 前端工作台 Phase 1（CM6 只读+merge diff+文件树）→ LSP 多语言注册表 → 前端 Phase 2 编辑

## 6. 实施记录

> 2026-08-07 决策点 D1-D7 全部落地并测试通过。下表为每个决策对应的实现文件与工具（符号均经代码验证）。

| 决策 | 实现文件 | 工具 / API |
|------|---------|-----------|
| D1 语义化编辑双原语 | `core/src/executor/hashline.rs`（锚点）+ `edit.rs`（apply_edit）+ `patch.rs`（apply_patch） | 工具 `apply_edit` / `apply_patch`；`Action::ApplyEdit` / `Action::ApplyPatch`（`executor/types.rs`，审批 Medium） |
| D2 diff 库 similar | `executor/patch.rs`（`FUZZY_RATIO_THRESHOLD = 0.75` fuzzy seek）；`snapshot/mod.rs::diff()` | 前端 diff 面板（`/workspace` 页：Phase 1 以带 +/- 着色的 `<pre>` 渲染 unified 文本；`@codemirror/merge` 依赖已引入，待 Phase 2 后端返回 old/new 双侧内容时启用） |
| D3 快照升级 | `core/src/snapshot/mod.rs`：gzip（`GZIP_MAGIC` `0x1f 0x8b`，legacy 向后兼容）、`gc()` 标记-清除（`GcStats`）、`diff()`（similar → `DiffResult`/`FileDiff`） | `/api/v1/workspace/diff?base=snapshot&session_id=&index=` |
| D4 代码智能层 | `core/src/lsp/`：`registry.rs`（`ServerSpec` + `BUILTIN_SERVERS`：rust-analyzer / typescript-language-server / pyright-langserver / gopls）、`client.rs`（自研 JSON-RPC 2.0 客户端）、`diagnostics.rs`（`LspManager`）；`executor/verification.rs`（`StructuredDiagnostic` / `parse_json_diagnostics`）；`executor/symbols.rs`（tree-sitter 多语言 `symbol_outline`） | 工具 `lsp`（goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation）；`verify_build`（结构化诊断）；`symbol_outline` |
| D5 前端工作台 | `gui-vite/src/components/workspace/`（`WorkspacePanel.tsx` react-arborist + CodeMirror 6、`WorkspaceTree.tsx`）；后端 `server/src/api/workspace/` | `/workspace` 路由（`App.tsx`）；`/api/v1/workspace/tree` `/read` `/diff`；api-client `fetchWorkspaceTree` / `fetchWorkspaceRead` / `fetchWorkspaceDiff` |
| D6 测试发现 | `core/src/executor/test_discovery.rs`（`discover_tests` / `parse_test_output` / `resolve_test_command` / `run_tests_action`） | 工具 `discover_tests`；`run_tests` 改造（framework / filter / suite） |
| D7 分期 P0–P2 | P0/P1 全部落地；P2：workspace API + 前端工作台 Phase 1（只读）+ LSP 注册表落地；前端 Phase 2 编辑按 D5 渐进式设计留待后续（2026-08-07） | 工具注册点：`core/src/agent/tool_registry/mod.rs::register_builtin_tools()` |

差距项补充落地：G3 文件浏览 → `executor/fs.rs`（`execute_glob` / `execute_list_dir`，`MAX_GLOB_RESULTS = 200`）+ 工具 `glob` / `list_dir` + read_file 目录模式；G4 read_file 增强（offset/limit、hashline 前缀 `N#ID|content`、截断消息、二进制嗅探）；G5 search_code 增强 → `executor/search.rs`（`SearchOptions` / `OutputMode` / `execute_search_code`）；统一截断层 → `executor/truncate.rs`（`MAX_LINES = 2000` / `MAX_BYTES = 50KB`）。

## 附录 A：参考项目

- opencode (anomalyco/opencode) — 源码级参考（工具/截断/LSP/git 四方面最完整）
- Claude Code — 工具 schema 官方类型 + 行为文档（闭源，社区逆向佐证）
- Continue.dev — multiEdit 原子性 + 缩进重投影
- Aider — 多策略降级 + /undo + git 工作流
- OpenAI Codex — apply_patch 工程化（Rust，与天演同栈）
- Helix / Zed — Rust LSP 客户端生产级参考
- mcpls — Rust LSP-over-MCP 现成实现
