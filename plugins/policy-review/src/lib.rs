//! policy-review 插件：评审策略（LLM-as-Judge 的**实现**，ADR-049 期 3-T3）。
//!
//! 本插件今天只剩**一个**评审面——「再叫一次模型，让它判断产物好不好」：
//!
//! **后台任务结果自审**（G4）：`LlmTaskReviewer`。任务完成通知（ADR-013）注入父会话
//! 前做一次轻量 LLM 自审，未通过则在结果前标记 `[自审未通过] {reason}`，由主 agent
//! 下一轮决策（**不自动重跑**）。原在 `core/src/executor/judge.rs`（约 200 生产行）。
//!
//! ## 旧范式已退役：「技能质量评审」（ADR-049 期 4c）
//!
//! 本插件曾有第二个评审面 `SkillReviewer`（技能使用复审，`reviewer.rs` 300 行 +
//! `reviewer_tests.rs` 164 行）：基于会话证据（`call_skill` 调用参数 + 工具结果 +
//! 后续用户反馈）给一次技能使用打分 0-10、判 `positive` / `neutral` / `negative`，
//! 落 VFS `skill/_reviews/<id>.jsonl`，再由 `SkillReviewPort::latest_review` 读给技能
//! 面板。**期 4c 整体删除**——它与 ADR-017 已经删掉的机械评分（Jaccard / G2b /
//! verify_preference）是同一类旧范式：「LLM 算一个分数，再让读侧展示这个分数」。
//!
//! **为什么当初保留未删**（期 3-T3 的判据，留档）：当时它被判为「未接线」而非死代码
//! ——读侧已接（技能面板能读评审），写侧未接（ADR-017 删除「记忆提取任务」时，
//! `review_conversation` 原本顺路挂的调用点随之消失）。删掉等于把「技能质量反馈」
//! 这条路彻底封死，而接线点当时还没有定论。
//!
//! **为什么现在能删**（三条同时成立，缺一条都不该动手）：
//!
//! 1. 写侧**始终零生产者**——生产环境里 `skill/_reviews/*.jsonl` 只可能来自开发期
//!    测试，所以读侧永远读到空（面板从来没有真的显示过一个分数）；
//! 2. 它的**输入**是含 assistant `tool_calls` 的会话原文，恰好是 ADR-017 §2.1 刻意
//!    排除在演化材料之外的东西——留着它等于留着一条绕过那条裁决的旁路；
//! 3. 技能质量判断**已经有了新的归属**（见下一条），不需要预先算出分数。
//!
//! **质量判断现在归谁**：演化智能体（`plugins/task-evolution`）。它依据「**技能使用
//! 统计**（`usage_stats` 的 `skill:` 前缀计数）+ **用户原话材料** + **技能清单**」
//! 自主判断哪个技能该改、该拆、该退役。判断是综述的产物，分数是旧范式的产物——后者
//! 需要一条独立的生产管道 + 一个装配插槽，前者零新增管道（材料即已有物）。
//!
//! **退役后的残留**：常量 `tianyan::skills::REVIEWS_PREFIX` 仍留在 core（技能发现时
//! 排除 `_reviews` 目录），否则存量目录会在某天被当成一个叫 `_reviews` 的技能列出来。
//! 它与本插件再无关系——本插件不再写那个目录。
//!
//! ## 为什么搬出 core（ADR-049 D7：按「变化原因」切）
//!
//! 剩下的东西**全是 B 级策略**（ADR-049 D3）：评审提示词文案、`VERDICT:` / JSON 的
//! 解析口径、分数分配、截断预算（2000 / 300 / 400 字符）、兜底默认通过。换这些的理由
//! 与换内核机制的理由毫无关系——调一句提示词、换评审模型、改一条判定标准，此前都要
//! 重编 core（42.4K 生产行）+ server（13.4K）。
//!
//! 搬走之后只重编本 crate + 装配包（`app` / `tauri`），内核与通道层不动。
//!
//! ## 为什么契约留 core（**不是**遗漏）
//!
//! | 留 core 的东西 | 位置 | 为什么不能搬 |
//! |---|---|---|
//! | `TaskReviewer` / `Judgment` / `Verdict` | `tianyan::executor::review` | 内核以 `Arc<dyn TaskReviewer>` 消费（`agent/background.rs` 任务管理器、`agent/tool_registry/mod.rs`、`AgentBuilder::with_task_reviewer`）；`Judgment` 出现在 trait 签名里。搬进插件 ⇒ 内核反向 `use tianyan_policy_review::…`，破 D2-1/D2-3 |
//!
//! 技能复审的契约（`SkillReviewPort` / `SkillReview`）**已随旧范式一并删除**（期 4c），
//! 不再有「留 core 还是搬插件」的问题；`REVIEWS_PREFIX` 单独留下，理由见上节。
//!
//! ## 为什么 `common/llm_judge.rs` **不能**搬进来
//!
//! `tianyan::common::llm_judge`（`parse_llm_json` / `truncate_output`）只有 3 个纯函数，
//! 但 6 个消费者里**4 个不是评审**：内核 `vfs/summary/engine.rs`（摘要引擎解析）、
//! `agent/background.rs`（任务通知摘要截断）、`agent/tool_registry/evolution_ops.rs`
//! （演化综述）、`plugins/tools-web`（抓取正文截断）。移进本插件会让内核反向依赖
//! 策略插件（D2-1/D2-3），因此它**原样留 core 的工具面**（与 `contracts.md` 附录 A
//! 一致）。本插件只**用**它（`use tianyan::common::llm_judge::…`）。
//!
//! ## 降级语义（未装配插槽时）
//!
//! 装配层不注入 `TaskReviewerFactory` 时：`AgentBuilder` 拿到 `None` →
//! `config.background_self_review` 为真时也**不装配自审器**，任务完成通知**直接投递**
//! （不带 `[自审未通过]` 标记），`build` 里 `warn!` 一条指路。该开关默认关闭
//! （opt-in，每任务多一次模型调用），缺实现的后果是「少一层质量复核」，比存储降级
//! （`error!`，「重启即丢数据」）轻一个量级 —— 与 T1 审批降级同一档。
//!
//! 这条降级**不静默**：装配期有 `warn!`，并由装配测试锁定（未装配 → 插槽为空）。
//!
//! ## 测试
//!
//! 迁移 9 条（4 条 G4 自审 + 3 条判定解析 + 2 条截断预算），逐个同名同断言；另有 1 条
//! **装配门禁**（工厂产出的自审器对空结果不花调用）。技能复审那 2 条会话证据评审用例与
//! 2 条技能端口门禁随旧范式删除（ADR-049 期 4c）。
//!
//! **源码纪律**：本 crate 的 `judge.rs` 只允许 `tianyan::…` 绝对路径，不得出现
//! `crate::…`（本 crate **没有** core 单测的 `#[path]` 同源包含——评审实现不被任何 core
//! 用例需要：core 的 `background` 自审用例用的是自己的 `FixedReviewer` 替身，锁的是
//! 「未通过 → 加前缀」这条内核侧语义）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod judge;

pub use judge::LlmTaskReviewer;

use std::sync::Arc;

use tianyan::executor::review::TaskReviewer;
use tianyan::model::ChatService;

/// 构造后台任务结果自审器（**装配期唯一入口**）。
///
/// 参数是装配层已经拿好的两样东西：**共享的**聊天客户端（`ModelServices.chat` 的
/// 克隆，不重复建 HTTP 客户端池）与评审模型名（`resolve_chat_model` 的结果）。
///
/// 为什么形状这么薄：判定提示词、解析口径与兜底都是本插件的私有细节，装配层不该知道；
/// 而「用哪个模型」是装配层的决定（配置解析 + 模型规格联动），插件也不该自己去读配置。
/// 于是这条边界正好落在「两个入参、一个 trait 对象」上——三处装配（`app` / `tauri` /
/// 集成测试）只各写一行类型擦除包装。
///
/// **不在这里判 `config.background_self_review`**：那是「要不要花这次调用」的成本
/// 开关（内核行为），判断留在 `AgentBuilder::build`；插件只回答「怎么审」。
pub fn task_reviewer(chat: Arc<dyn ChatService>, model: &str) -> Arc<dyn TaskReviewer> {
    Arc::new(LlmTaskReviewer::new(chat, model))
}

#[cfg(test)]
mod factory_tests {
    use super::*;
    use tianyan::model::MockChatService;

    /// 工厂产出的自审器必须**真的是** `LlmTaskReviewer` 的行为：空结果不花调用、
    /// 直接判通过（`times(0)` 锁住「零 LLM 调用」）。
    ///
    /// 判别力：若工厂返回的是一个「总是不通过」的桩（或把 `review` 写成恒 Fail），
    /// 空结果这条断言立刻变红。
    #[tokio::test]
    async fn test_task_reviewer_factory_short_circuits_empty_result() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().times(0);
        let reviewer = task_reviewer(Arc::new(mock), "test-model");
        let judgment = reviewer.review("任务", "   ").await;
        assert!(judgment.verdict.is_pass());
        assert_eq!(judgment.reason, "任务无结果输出");
    }
}
