# ADR-008: 快照升级 —— gzip 压缩、GC 与 diff 集成

**日期**: 2026-08
**状态**: ✅ 已采纳
**影响范围**: 存储 — `snapshot` 模块（`core/src/snapshot/mod.rs`），扩展 ADR-006

---

## 背景

ADR-006 确立快照为 VFS 例外的独立文件存储（内容寻址对象库 + 树文件 + redo 增量）。编程助手场景（gap-analysis G2/G6，决策 D3）对快照提出三点增强需求：

1. **压缩**：消息级快照存储全量工作区文件镜像，未压缩对象库随使用持续膨胀
2. **GC**：树文件与 redo 增量不断累积，无清理机制，redo 孤儿 cache 文件长期滞留
3. **diff**：回退/重做缺少变更可视化，前端工作台需要 diff 数据源（D2 已选型 `similar`）

同时评估了 git 对象层替代方案（opencode `git write-tree` / Codex ghost commit 模式，见 gap-analysis §4.5）。

## 决策

**快照纯自研升级，保留 ADR-006 存储结构与 sha256 内容寻址，不引入 git 二进制 / git2 / gix。**

1. **gzip 压缩**（`flate2`，默认压缩级别）：对象库字节以 gzip 流写入。对象键**不变**，仍为 raw 内容 sha256（`hex_sha256`），跨会话全局去重语义保持不变。读取时以 gzip 魔数 `0x1f 0x8b`（`GZIP_MAGIC`）识别：命中则 `GzDecoder` 解压，否则按 legacy 未压缩对象处理。旧版本快照库向后兼容（有专门测试覆盖）。
2. **`gc()` 标记-清除**：标记阶段收集所有会话的 `trees/*.json` 与 `redo/tree-*.json`（可达集），清除孤儿 redo cache 文件与不可达对象，返回 `GcStats`。
3. **diff 集成**：`diff(session_id, index)` 经 `similar::TextDiff::from_lines` 生成 `DiffResult` / `FileDiff` / Hunk（added / removed / modified / binary 分类 + unified 文本），服务端 `/api/v1/workspace/diff?base=snapshot` 与前端 diff 面板共用同一数据源。
4. **否决 git 影子仓库路线**（D3）：
   - 否决影子仓库：依赖 git 二进制假设，与"全能型通用代理"定位冲突，非 git 仓库无法工作
   - 否决 gix：restore 编排未完成，工程不成熟
   - 否决 git2：C 编译负担 + 生态逆风（cargo 正讨论从 libgit2 迁移 CLI、turborepo 已移除 git2）
   - 自研升级非 git 仓库天然支持，零环境依赖

## 后果

### 正面
- 对象库体积显著下降（文本工作区压缩比高），sha256-of-raw 键不变保证既有快照与去重语义完好
- GC 控制存储膨胀，redo 孤儿 cache 不再滞留
- diff 能力打通回退可视化与前端工作台数据源
- 仅新增轻量依赖（flate2 / similar），不引入 git 工具链假设

### 负面 / 代价
- 读对象需魔数判断 + 可能解压，读写路径微增 CPU
- 压缩对象对故障排查（直接查看对象文件）不友好
- GC 需扫描快照目录全量树文件，大目录首次执行耗时

### 边界条件（违反即重新评估）
- 对象键必须保持 raw 字节 sha256（去重语义依赖），不得改为压缩后字节的哈希
- 魔数识别失败（非 gzip 前缀）一律按 legacy 未压缩对象处理，不得静默破坏
- GC 不得删除仍被任何会话树引用的对象；redo 清理仅限孤儿 cache 文件
- 继续遵守 ADR-006 边界：`snapshots/` 仅存工作区快照数据，不得扩展为通用存储

## 关键文件

- `core/src/snapshot/mod.rs` — `SnapshotManager`：`capture_file`（gzip 写路径）、`gc()`（`GcStats`）、`diff()`（`DiffResult` / `FileDiff`）、`GZIP_MAGIC`
