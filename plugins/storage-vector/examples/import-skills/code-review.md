---
id: code-review
abstract: 双轴代码审查——Standards（仓库标准 + 气味基线）与 Spec（对照原始意图）并行子代理审查、并列报告 | 适用场景: 审查分支/工作区改动、发布前审查、"review since X"
---
# 双轴代码审查（Code Review）

**ID**: `code-review`

**版本**: 1.0.0

**描述**: 对"从固定点以来的 diff"做两轴审查（标准符合性 + 规格忠实性），双轴可并行子代理执行。

**状态**: 正式

## 适用场景

- 用户想审查一个分支、一处改动、工作区 diff，或说"review since X"
- 发布前的质量把关（配合全量门禁）
- 大改动的独立复核（子代理执行、避免上下文污染）

## 使用说明

### 两轴

- **Standards 轴**——代码是否符合本仓库**成文的标准**（AGENTS.md 硬约束、编码规范、既有惯例）？
- **Spec 轴**——代码是否忠实实现了**原始意图**（issue / 方案文档 / 用户需求）？

两轴**并行子代理**执行（互不污染上下文），主 agent 汇总并列报告。

### 流程

1. **固定比较点**：用户给什么就是什么（commit / branch / tag / HEAD~N）；未给则询问。取 diff：`git diff <fixed-point>...HEAD`（三点，对 merge-base 比较）+ `git log <fixed-point>..HEAD --oneline`。先确认 fixed-point 可解析、diff 非空。
2. **找 Spec 来源**：commit 消息里的引用 → 用户给路径 → `docs/` 或规格文件 → 都没有则问用户；确实没有时 Spec 轴跳过并报"无 spec 可用"。
3. **找 Standards 来源**：仓库里一切"代码该怎么写"的文档（AGENTS.md、docs/architecture/、重构实践指南）。**仓库标准始终优先**。
4. **气味基线**（仓库没有明文时兜底；每条是"可能的 X"判据，不是硬性违规；工具已强制的跳过）：

   | 气味 | 修法方向 |
   |---|---|
   | Mysterious Name（名字不揭示做什么） | 重命名；起不出诚实的名字 = 设计浑浊 |
   | Duplicated Code（同形逻辑多 hunk 出现） | 提共享形状、两处调用 |
   | Feature Envy（方法伸进别的对象的数据） | 把方法搬到它羡慕的数据上 |
   | Data Clumps（同几个字段总一起旅行） | 捆成一个类型 |
   | Primitive Obsession（用原始类型充当领域概念） | 给概念一个小类型 |
   | Repeated Switches（同类型的 switch 反复出现） | 多态或共享映射 |
   | Shotgun Surgery（一处逻辑改动散到多文件） | 把"一起变的东西"聚到一个模块 |
   | Divergent Change（一个文件因无关原因被反复改） | 拆分，让每个模块只因一个原因变 |
   | Speculative Generality（为不存在的需求加抽象） | 删掉，内联回去 |
   | Message Chains（长 `a.b().c().d()` 导航） | 在第一个对象上隐藏整条走链 |
   | Middle Man（只做纯转发的类 / 函数） | 删掉，直接调真目标 |
   | Refused Bequest（子类忽略继承来的大部分东西） | 去继承、用组合 |

5. **并行执行两轴子代理**（各带：比较点、diff、来源文档、输出格式——文件:行号 + 分级 + 建议）。
6. **汇总并列报告**：Standards 发现 + Spec 发现 + 交叉项（两轴都指到的改动优先复核）。

### 输出规范（建议）

- 每条：`文件:行号` + 气味 / 偏差名 + 一句话证据 + 修法方向；
- 分级（高 / 中 / 低）+ 明确"这是判断项不是硬性"；
- 只报**本次 diff 引入**的问题（不翻旧账，除非改动放大它）。

## 来源

- 外部标准化技能库移植（`~/.agents/skills\code-review`），2026-09-14 导入
- 气味清单出处：Martin Fowler《Refactoring》第 3 章（社区技能引用版）
