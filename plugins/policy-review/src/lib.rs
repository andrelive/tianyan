//! policy-review 插件：评审策略（LLM-as-Judge 的**实现**，ADR-049 期 3-T3）。
//!
//! 两个评审面，共用同一种形状——「再叫一次模型，让它判断产物好不好」：
//!
//! 1. **后台任务结果自审**（G4）：`LlmTaskReviewer`。任务完成通知（ADR-013）注入父会话
//!    前做一次轻量 LLM 自审，未通过则在结果前标记 `[自审未通过] {reason}`，由主 agent
//!    下一轮决策（**不自动重跑**）。原在 `core/src/executor/judge.rs`（约 200 生产行）。
//! 2. **技能使用复审**：`SkillReviewer`。基于会话证据（调用参数 + 工具结果 + 后续用户
//!    反馈）给技能使用效果打分（0-10），落 VFS `skill/_reviews/<id>.jsonl`。
//!    原在 `core/src/skills/reviewer.rs`（275 行）。
//!
//! ## 为什么搬出 core（ADR-049 D7：按「变化原因」切）
//!
//! 这两个文件里的东西**全是 B 级策略**（ADR-049 D3）：评审提示词文案、`VERDICT:` /
//! JSON 的解析口径、评分标准的分数分配、证据窗口大小（前后各几条）、截断预算
//! （2000 / 300 / 400 字符）、每会话最多评审次数（3）、保留最近 50 条、兜底默认通过。
//! 换这些的理由与换内核机制的理由毫无关系——调一句提示词、换评审模型、改一条评分
//! 标准，此前都要重编 core（42.4K 生产行）+ server（13.4K）。
//!
//! 搬走之后只重编本 crate + 装配包（`app` / `tauri`），内核与通道层不动。
//!
//! ## 为什么契约留 core（**不是**遗漏）
//!
//! | 留 core 的东西 | 位置 | 为什么不能搬 |
//! |---|---|---|
//! | `TaskReviewer` / `Judgment` / `Verdict` | `tianyan::executor::review` | 内核以 `Arc<dyn TaskReviewer>` 消费（`agent/background.rs` 任务管理器、`agent/tool_registry/mod.rs`、`AgentBuilder::with_task_reviewer`）；`Judgment` 出现在 trait 签名里。搬进插件 ⇒ 内核反向 `use tianyan_policy_review::…`，破 D2-1/D2-3 |
//! | `SkillReviewPort` / `SkillReview` | `tianyan::skills` | `SkillReviewPort` 是通道层的读端口（唯一真实调用点 `server/src/api/skills/handlers.rs` 的技能统计）；`SkillReview` 出现在其签名里 |
//! | `REVIEWS_PREFIX` | `tianyan::skills::REVIEWS_PREFIX` | **两处必须同源**：插件的落盘路径（`skill/_reviews/<id>.jsonl`）与内核技能发现时的过滤条件（`SkillManager` 不得把 `_reviews` 列成一个技能） |
//!
//! `SkillReviewPort` 的方法面按**调用点证据**最小化到 **1 个**（`latest_review`）。
//! **产生评审的完整逻辑 `review_conversation` 没有进契约**——见下节。
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
//! ## 未接线遗留：`review_conversation`
//!
//! `SkillReviewer::review_conversation`（约 160 行：会话 JSONL → 提取 `call_skill`
//! 样本 → 逐样本 LLM 评分 → 追加落盘）今天**零生产调用者**（只有本 crate 的测试）。
//! 它是**未接线**而非死代码：GUI 的技能面板能读评审（`SkillReviewPort::latest_review`），
//! 却永不产生新评审——读侧已接、写侧未接。ADR-017 收敛后「记忆提取任务」被删，它原本
//! 顺路调用的挂点随之消失，评审产出链因此断开。
//!
//! 处置（刻意的）：保留在 `SkillReviewer` 具体类型上，**不进 `SkillReviewPort`**（契约里
//! 放一个零调用的方法，等于让未来的实现者对着空气实现）、**不删**（删掉等于把「技能
//! 质量反馈」这条路彻底封死）、**不改 mock**。ADR-049 记了一条待接线遗留，接线点应选在
//! 「会话收尾 / 演化任务」这类**已有模型调用**的位置，避免额外轮次。
//!
//! ## 降级语义（未装配插槽时）
//!
//! 装配层不注入 `TaskReviewerFactory` / `SkillReviewFactory` 时：
//!
//! - **自审**：`AgentBuilder` 拿到 `None` → `config.background_self_review` 为真时也
//!   不装配自审器，任务完成通知**直接投递**（不带 `[自审未通过]` 标记），`build` 里
//!   `warn!` 一条指路。该开关默认关闭（opt-in，每任务多一次模型调用），缺实现的后果是
//!   「少一层质量复核」，比存储降级（`error!`，「重启即丢数据」）轻一个量级 ——
//!   与 T1 审批降级同一档。
//! - **技能评审读端口**：`AppState::create_skill_reviewer` 返回 `None`，
//!   `GET /api/v1/skills/stats` 的 `reviews` 字段为空数组（响应结构不变）。今天
//!   「没有评审器」与「有评审器但没有记录」在该接口上本来就不可区分（两者都是空数组），
//!   所以这条降级不改变可观测形状，只是不再凭空造一个实现。
//!
//! **两处降级都不静默**：自审缺失有装配期 `warn!`；读端口缺失由
//! `AssemblyServices` 的插槽注释与装配测试锁定。
//!
//! ## 测试
//!
//! 迁移 11 条（judge 9：4 条 G4 自审 + 3 条判定解析 + 2 条截断预算；reviewer 2 条会话
//! 证据评审），逐个同名同断言；另有 3 条**装配门禁**（工厂产出可用的端口 / 空 VFS 回
//! `None` / 空结果不触发 LLM）。core 侧 `--lib` 因此只少 11 条（判据与提示词跟着实现走）。
//!
//! **源码纪律**：本 crate 的 `judge.rs` / `reviewer.rs` 只允许 `tianyan::…` 绝对路径，
//! 不得出现 `crate::…`（本 crate **没有** core 单测的 `#[path]` 同源包含——评审实现
//! 不被任何 core 用例需要：core 的 `background` 自审用例用的是自己的 `FixedReviewer`
//! 替身，锁的是「未通过 → 加前缀」这条内核侧语义）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod judge;
mod reviewer;

pub use judge::LlmTaskReviewer;
pub use reviewer::{SkillReviewer, SKILL_REVIEW_PROMPT};

use std::sync::Arc;

use tianyan::executor::review::TaskReviewer;
use tianyan::model::ChatService;
use tianyan::skills::SkillReviewPort;
use tianyan::vfs::VirtualFileSystem;

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

/// 构造技能使用复审器（**装配期唯一入口**，经 [`SkillReviewPort`] 类型擦除）。
///
/// 第三个参数是 VFS：评审记录与技能摘要都落在 `tianyan://skill/` 命名空间里，必须是
/// 与 `SkillManager` **同一个** VFS 实例，否则面板读到的与评审写下的会是两套内容。
pub fn skill_reviewer(
    chat: Arc<dyn ChatService>,
    model: &str,
    vfs: Arc<dyn VirtualFileSystem>,
) -> Arc<dyn SkillReviewPort> {
    Arc::new(SkillReviewer::new(chat, vfs, model.to_string()))
}

#[cfg(test)]
mod factory_tests {
    use super::*;
    use tianyan::common::types::{ContentLevel, ContextNamespace, TianyanUri};
    use tianyan::model::MockChatService;
    use tianyan::test_utils::MockVfs;

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

    /// 技能评审端口必须从**插件写入的路径**读得出记录（`skill/_reviews/<id>.jsonl`
    /// 的最后一条 = 最新）；空 VFS → `None`（而不是报错或伪造一条）。
    ///
    /// 判别力：路径前缀（`REVIEWS_PREFIX`）若与 `SkillManager` 的过滤口径分叉，或端口
    /// 实现改成「取第一条」，两条断言都会红。
    #[tokio::test]
    async fn test_skill_review_factory_reads_latest_from_reviews_jsonl() {
        let vfs = Arc::new(MockVfs::new());
        let uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec!["_reviews".to_string(), "planning.jsonl".to_string()],
        );
        let older = r#"{"ts":1,"skill_id":"planning","session_id":"s1","score":3,"verdict":"negative","user_feedback":"none","reason":"旧","evidence":"e"}"#;
        let newer = r#"{"ts":2,"skill_id":"planning","session_id":"s2","score":9,"verdict":"positive","user_feedback":"positive","reason":"新","evidence":"e"}"#;
        vfs.set_content(&uri, ContentLevel::Detail, &format!("{older}\n{newer}"));

        let port = skill_reviewer(
            Arc::new(MockChatService::new()),
            "test-model",
            vfs.clone() as Arc<dyn VirtualFileSystem>,
        );
        let latest = port.latest_review("planning").await.unwrap();
        let latest = latest.expect("应有记录");
        assert_eq!(latest.ts, 2, "应取最后一条（最新）");
        assert_eq!(latest.score, 9);

        // 空 VFS（未产生过评审）→ None，不报错
        assert!(port
            .latest_review("never-reviewed")
            .await
            .unwrap()
            .is_none());
    }

    /// 端口实现必须与具体类型**同一份读取逻辑**（trait 方法委托给固有方法）。
    ///
    /// 判别力：若有人把 `SkillReviewPort::latest_review` 另写一份（例如漏掉 JSONL
    /// 逐行解析），它与 `SkillReviewer::load_reviews` 的结果就会分叉；
    /// 此测试用同一实例的两种调用面比对。
    #[tokio::test]
    async fn test_skill_review_port_delegates_to_concrete_type() {
        let vfs = Arc::new(MockVfs::new());
        let uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec!["_reviews".to_string(), "writer.jsonl".to_string()],
        );
        vfs.set_content(
            &uri,
            ContentLevel::Detail,
            r#"{"ts":7,"skill_id":"writer","session_id":"s1","score":6,"verdict":"neutral","user_feedback":"mixed","reason":"一般","evidence":"e"}"#,
        );
        let concrete = SkillReviewer::new(
            Arc::new(MockChatService::new()),
            vfs.clone() as Arc<dyn VirtualFileSystem>,
            "test-model".to_string(),
        );
        let through_port = SkillReviewPort::latest_review(&concrete, "writer")
            .await
            .unwrap();
        let through_inherent = concrete.latest_review("writer").await.unwrap();
        assert_eq!(
            through_port.map(|r| r.ts),
            through_inherent.map(|r| r.ts),
            "契约方法必须委托给同一份读取逻辑"
        );
    }
}
