# ADR-033: 安全策略收敛——默认自主（ApprovalMode 三态）

**日期**: 2026-09-12
**状态**: ✅ 已采纳
**影响范围**: `core/src/config/security.rs`（SecurityConfig 默认值与字段）、`core/src/executor/approval/`（workflow / types）、`core/src/agent/builder.rs`（审批装配）、server 配置 API、GUI 设置页、`config.example.toml`

---

## 背景

安全/审批相关的配置开关长期增殖，语义重叠且互作用不清：

| 开关 | 现状 | 问题 |
|------|------|------|
| `safety_mode`（Strict/Transform/Permissive） | 命令层策略（阻断/重写/放开）——有作用 | 与审批层的关系无文档 |
| `enabled` | 安全总开关 | 与各子开关关系模糊 |
| `confirm_commands` | **无任何生产消费点** | 死开关 |
| `wait_for_approval` | 审批层：挂起等待 GUI 面板 | 与 `allow_all_operations` 同轴冲突 |
| `prompt_commands` | 审批层：强制人工 | 保留（细粒度补充） |
| `allow_all_operations` | 审批层 + 命令层**双击穿**：审批全放行 + 命令检查跳过元字符/解释器 | 一个开关横跨两层，语义混淆（收敛的主要动因） |
| `unattended_mode` | **无配置入口**（仅测试可开） | 死配置 |

实测证据（2026-09-12 排查）：

- 运行时装配的规则链为：`Deny(黑名单) → RiskBelow(Medium) 自动放行 → 安全 → Critical 默认拒绝 → 无人值守（不可达）→ 指纹确认 → 挂起（默认关）→ 拒绝降级询问`；
- 默认行为（`Confirm` 链路）为"未确认的危险操作**拒绝**并降级为 `ask_user` 追问"——对本地个人 agent 场景构成打扰，与产品定位（自主行动）不符；
- 项目所有者实际已用 `allow_all_operations = true` 运行（配置实证），即"全自主"是真实使用形态；
- `submit_result` 类问题的根因是"同一语义多份数据表达"，本 ADR 以同种思路收敛审批层：**审批行为 = 一个枚举（单一事实源）**。

## 决策

### 1. 审批行为收敛为单枚举 `ApprovalMode`

```rust
/// 审批行为模式（审批层单一事实源）。
pub enum ApprovalMode {
    #[default]
    Autonomous,   // 全自主：黑名单外全放行（主/子代理一致）；审计记录
    Confirm,      // 确认式：危险操作拒绝 → "询问用户"降级链路（迁出默认）
    Interactive,  // 交互式：危险操作挂起等待 GUI 审批面板（原 wait_for_approval=true）
}
```

- **默认值 = `Autonomous`**：新安装与未显式配置的既有安装，升级后即为全自主（黑名单仍强制生效）；
- 迁移规则（旧配置读取归一，见 §3）：
  - `wait_for_approval = true` → `Interactive`（保留其意图）；
  - 其余（含 `allow_all_operations = true/false`、全缺省） → `Autonomous`（新默认；`true` 与默认等价，`false` 视为未表态）；
  - `approval_mode` 非默认值（= 显式设置）时不被旧字段覆盖（冲突时忽略旧字段并告警）。

### 2. 评估链单点：模式短路（替代"规则注入"）

`ApprovalWorkflow` 的评估顺序改为：

```
① Deny 规则（blocked_commands 黑名单）——永远最优先，任何模式不可覆盖
② forced_prompt（prompt_commands 命中）——跳过一切自动放行
③ mode 短路：
   Autonomous → Approve（记录审计）
   Confirm    → 落到"拒绝并降级询问用户"链路
   Interactive→ 落到"挂起等待人工"链路（子代理路径不允许等待 → 立即拒绝）
④ 细粒度规则（自动规则 / Safe / Critical 默认拒绝 / 指纹确认）
```

要点：
- "模式"从**隐式规则表顺序**升为**显式一等分支**（可读、可测、单点）；
- `builder.rs` 不再注入 `allow_all_operations` 规则（删除该段）——模式语义由 workflow 自身表达；
- `prompt_commands` 语义不变：任何模式下命中的命令都不被自动放行（Autonomous 下落入确认链路）。

### 3. 旧字段迁移（读取兼容，不再写出）

`SecurityConfig` 保留两个**已弃用读取字段**（`allow_all_operations`、`wait_for_approval`），仅在反序列化时接受，序列化时不再输出：

- 加载后经 `SecurityConfig::normalize()` 单点归一为新 `approval_mode`（load 路径调用，与 `expand_user_dir` 同层）；
- 下次写回配置时旧字段自然消失（自动迁移，对齐 ADR-024「cron 配置自动换算」先例：不逼用户手工迁移）。

### 4. 死开关清理

- 删除 `confirm_commands`（无消费点）；
- 删除 `unattended_mode`（无配置入口；其"Medium/High 自动批准"语义已被 `Autonomous` 覆盖）；
- 删除 `SecurityConfig.allow_all_operations`（跨层双击穿，见 §6）。

### 5. 主/子代理差异归位：mode × 等待能力的函数

- `request_approval`（主，可等待）/ `request_approval_no_wait`（子，不可等待）**保留**——这是能力差异；
- 差异行为由模式决定且**单点定义在 workflow**：
  - `Autonomous`：两者一致（都放行，无差异）；
  - `Confirm`：子代理立即拒绝上报（ADR-011 语义），主代理降级询问；
  - `Interactive`：主代理挂起等待，子代理立即拒绝。

### 6. 命令层收敛：`SafetyMode` 四态（吸收 `allow_all_operations` 命令层语义）

实施中发现 `allow_all_operations` 同时被**命令检查层**消费（`executor::security::check_command`：跳过元字符/解释器检查）——为消除"一个开关横跨两层"，其命令层语义并入 `SafetyMode`：

| 态 | 语义 |
|----|------|
| `Strict` | 元字符/解释器检查 + 黑名单全生效 |
| `Relaxed`（**新默认**） | 跳过元字符/解释器检查（链式命令与解释器可用），**黑名单仍强制**——原 `allow_all_operations` 命令层语义 |
| `Transform` | 危险命令重写为安全等价操作（回收站） |
| `Permissive` | 跳过一切检查（含黑名单） |

迁移：`allow_all_operations = true` 且 `safety_mode = strict` → `safety_mode = relaxed`（保留旧行为）；已是 Transform/Permissive（更放开）不覆盖。

层级关系：`SafetyMode`（检查层）与 `ApprovalMode`（审批层）**正交**；`blocked_commands` 黑名单在**两层都强制**（命令层命中即拒绝；审批层 Deny 规则最优先）。

## 后果

**正面**：
- 审批行为一个枚举说清：默认全自主、零打扰；想回到"问一下"只需 `approval_mode = "confirm"`（或对特定命令配 `prompt_commands`）；
- 消除 5 个开关的语义重叠与 2 个死开关；
- "模式"显式化后，测试可直接按模式断言（Autonomous/Confirm/Interactive 三组行为测试）；
- 主/子代理审批差异从"两条执行路径各自实现"收敛为"模式 × 等待能力"的单点函数——同类漂移（请求/执行不对称）不再有滋生空间。

**负面 / 边界**：
- **默认行为变更（两处）**：未显式配置的用户升级后 ① 审批从"确认式"变为"全自主"、② 命令检查从"严格"变为"放宽"（跳过元字符/解释器）——需在 RELEASE_NOTES 显著说明（0.x 阶段的可接受演进；提供 `approval_mode="confirm"` / `safety_mode="strict"` 回退路径）；
- 全自主后**黑名单成为唯一硬护栏**：`blocked_commands` / `blocked_directories` 的质量即安全性；文档需明确该责任边界；
- 不可逆操作（黑名单外）无第二道防线，依赖工作区快照回退与审计日志（`audit_logging` 保持默认开启）。

## 附：不在本 ADR 范围

- 黑名单跨平台升级（建议项：Windows 路径补全，属用户配置/示例模板，另行处理）；
- GUI 审批面板组件本身（保留；`pending` 为空时不渲染，天然满足"默认不出现"）。
