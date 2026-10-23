---
id: parallel-agent-orchestration
abstract: 多 agent 同工作区并行安全手册——全局 git 操作禁令、文件域分批、波次门禁、神秘回滚取证与恢复 | 适用场景: 并行子代理编辑代码、后台任务协作、文件被"神秘回滚"、git stash 恢复
---
# 并行 Agent 编排（同工作区安全手册）

**ID**: `parallel-agent-orchestration`

**版本**: 1.0.0

**描述**: 多个 agent 编辑同一 git 工作区的硬规则、波次门禁、"文件神秘回滚"取证决策树与恢复手册。

**状态**: 正式

## 适用场景

- 派出并行后台 agent / 子代理编辑代码之前
- 多个会话 / 任务共享同一工作区时
- 发现"文件被神秘回滚 / 修改"时（先内部、后外部）
- 从 git stash 恢复丢失的工作

## 使用说明

### 核心失败模式

**文件域不相交 ≠ git 操作不相交。** 两个 agent 可以永远编辑不同文件而无冲突——直到其中一个跑了**全局 git 操作**（`git stash` / `reset --hard` / `checkout .` / `clean`）。那一条命令会原子性地摧毁其他所有进行中的编辑，而其他 agent 继续"成功地"编辑一个幻影状态（编辑工具对着磁盘报成功、随即被回滚）。

实战教训：4 个并行 agent 做不相交文件的死代码清除；一个 agent 用 `git stash` 测"干净树能否编译"；另外 3 个 agent 的编辑瞬间蒸发；编排者浪费大量时间误诊"外部并发进程"，最后才发现两个 stash 都是内部产物。

### 并行执行硬规则

1. **全局 git 操作在 agent 提示词中一律禁止。** 绝不允 background / edit agent 运行：`git stash`、`git reset --hard`、`git checkout .`、`git clean`、`git restore .`。允许：`git diff`、`git status`、`git checkout <具体文件>`、`git log`、`git reflog`、`git show`。
2. **每次委托提示词中显式写出禁令**——agent 不会自己推断。一行即可："禁止运行 git stash/reset/checkout ./clean——全局 git 操作被禁止；只允许 diff/status/log/reflog。"
3. **按文件域分批，验证不相交**：启动并行波次前，确认没有文件出现在两个任务里。验证 / 测试命令可以共享（cargo 会在 target-dir 锁上自动串行）。
4. **波次之间设门禁**：每一波结束后，由编排者**亲自**跑全量质量门（fmt + lint + 测试）再启动下一波。绝不让 agent 相互验证对方改动的结果。
5. **需要验证"干净树"时，用 `git worktree` 或临时克隆**——绝不在共享树里 stash 切换。

### 取证："文件神秘回滚"决策树（先内部，后外部）

1. `git reflog -5`——找 `reset:` / `checkout:` 条目。`reset: moving to HEAD` 是内部 stash/reset 的铁证，不是外部行为。
2. `git stash list`——运行中产生的 stash 是内部产物。message 文本透露作者（`WIP on master` = 默认 `git stash`；自定义消息 = 某个 agent 起的名）。
3. `git status --short` + `git diff --stat`——对照最后一次已知完好的文件集。哪些文件从 modified 列表里消失 = 谁的工作丢了。
4. **完成 1-3 之后**再考虑外部进程（IDE watcher、其他会话）。检查会话列表——你自己的会话也在里面；"神秘并发会话"往往就是你自己。

### 恢复手册

- **从 stash 恢复单个文件而不 pop**（pop 会应用全部、可能砸掉其他 agent 的进行中工作）：
  `git checkout 'stash@{N}' -- <file1> <file2> ...`
- 用 `git status` 验证恢复集——每个预期文件必须重新显示为 modified。
- 如果 stash pop 已中途失败：把树重置到 HEAD（`git checkout -- .`），再在干净树上 `git stash apply`——**但必须先确认没有其他 agent 在编辑**（用波次门禁协调）。
- **最完整的 stash**（`git stash show --stat` 的超集最大者）通常包含所有人工作的并集——恢复时优先它。
- 恢复后：**重跑波次门禁**。不要假设恢复的树能编译。

### 本地实例（天演语境）

- 天演 `AGENTS.md`「协作纪律」即本手册的精简版（禁令清单）；本技能补齐**恢复手册**与**决策树**——两者配合使用。
- 天演子代理（`delegate_to_agent`）委托提示词应携带规则 2 的禁令行。

## 来源

- 外部标准化技能库移植（`~/.agents/skills\parallel-agent-orchestration`），2026-09-14 导入
- 与天演 AGENTS.md 协作纪律同源互补（禁令一致；本技能补恢复手册）
