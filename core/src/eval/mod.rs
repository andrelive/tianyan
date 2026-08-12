//! 回答质量评测（LLM-as-Judge 评分式）。
//!
//! 与 [`crate::executor::judge`]（工具执行验证门控，PASS/FAIL 二值）
//! 正交：本模块评测 **Agent 最终回答的质量**，输出 1-10 分维度评分。
//!
//! 定位：**离线评测能力**（golden 用例 + 批量运行），供质量回归与
//! 基准对比使用，不接入在线对话链路（每次评分额外消耗一次 LLM 调用，
//! 由调用方按需启用）。
//!
//! 可见性：`pub(crate)`——无运行时调用者，属保留工具模块；由外部评测
//! 驱动（CLI 或测试）按需启用时再升为 `pub`。
//!
//! 用法：
//! ```ignore
//! let judge = AnswerJudge::new(model_service, "gpt-4");
//! let eval = judge.evaluate("问题", "回答", None).await?;
//! println!("总分: {}", eval.overall_score);
//! ```

// 离线基准工具：pub(crate) 下无 crate 内调用者，模块整体豁免 dead_code /
// unused_imports（API 保留供外部评测驱动；激活时删除本豁免）。
#![allow(dead_code, unused_imports)]

mod golden;
mod judge;
mod runner;

pub use golden::golden_cases;
pub use judge::AnswerJudge;
pub use runner::{format_report, run_eval_suite, EvalCase, EvalResult};

/// 回答质量总体判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerVerdict {
    /// 优秀（总分 ≥ 8.5）。
    Excellent,
    /// 良好（总分 ≥ 7）。
    Good,
    /// 及格（总分 ≥ 5）。
    Fair,
    /// 差（总分 < 5）。
    Poor,
}

impl AnswerVerdict {
    /// 由总分（1-10）映射判定。
    pub fn from_score(score: f64) -> Self {
        match score {
            s if s >= 8.5 => Self::Excellent,
            s if s >= 7.0 => Self::Good,
            s if s >= 5.0 => Self::Fair,
            _ => Self::Poor,
        }
    }
}

impl std::fmt::Display for AnswerVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Excellent => write!(f, "优秀"),
            Self::Good => write!(f, "良好"),
            Self::Fair => write!(f, "及格"),
            Self::Poor => write!(f, "差"),
        }
    }
}

/// 回答质量的四个评测维度（各 1-10 分）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimensionScores {
    /// 相关性：回答是否切题、聚焦用户意图。
    pub relevance: f64,
    /// 正确性：事实是否准确、无错误信息。
    pub correctness: f64,
    /// 完整性：是否覆盖用户需求的所有关键点。
    pub completeness: f64,
    /// 清晰度：结构、表达、可读性。
    pub clarity: f64,
}

impl DimensionScores {
    /// 加权总分（1-10）：正确性 0.4 + 相关性 0.25 + 完整性 0.25 + 清晰度 0.1。
    pub fn weighted_overall(&self) -> f64 {
        self.correctness * 0.4
            + self.relevance * 0.25
            + self.completeness * 0.25
            + self.clarity * 0.1
    }
}

/// 单条问答的评测结果。
#[derive(Debug, Clone)]
pub struct AnswerEvaluation {
    /// 总分（1-10，加权）。
    pub overall_score: f64,
    /// 各维度评分。
    pub dimensions: DimensionScores,
    /// 回答的优点（LLM 提取，可为空）。
    pub strengths: Vec<String>,
    /// 改进建议（LLM 提取，可为空）。
    pub improvements: Vec<String>,
    /// 总体判定。
    pub verdict: AnswerVerdict,
    /// 解析是否成功（false 表示走了回退路径，分数不可靠）。
    pub parsed: bool,
}

impl AnswerEvaluation {
    /// 解析失败时的中性回退（5 分，`parsed = false`）。
    pub fn fallback(reason: &str) -> Self {
        Self {
            overall_score: 5.0,
            dimensions: DimensionScores {
                relevance: 5.0,
                correctness: 5.0,
                completeness: 5.0,
                clarity: 5.0,
            },
            strengths: Vec::new(),
            improvements: vec![reason.to_string()],
            verdict: AnswerVerdict::Fair,
            parsed: false,
        }
    }
}
