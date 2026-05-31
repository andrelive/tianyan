//! 内容加载模块。
//!
//! 基于相关性分数的内容加载策略。

use crate::common::types::ContentLevel;

/// 基于相关性分数的内容加载策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentLoadStrategy {
    /// 加载完整内容（L2） - 用于高相关性（分数 > 0.85）
    Full,
    /// 加载概览（L1） - 用于中等相关性（0.6 < 分数 <= 0.85）
    Overview,
    /// 仅加载摘要（L0） - 用于低相关性（分数 <= 0.6）
    Abstract,
}

impl ContentLoadStrategy {
    /// 根据相关性分数确定策略。
    pub fn from_score(score: f32) -> Self {
        if score > 0.85 {
            Self::Full
        } else if score > 0.6 {
            Self::Overview
        } else {
            Self::Abstract
        }
    }

    /// 获取此策略对应的内容层级。
    pub fn to_content_level(&self) -> ContentLevel {
        match self {
            Self::Full => ContentLevel::Detail,
            Self::Overview => ContentLevel::Overview,
            Self::Abstract => ContentLevel::Abstract,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_strategy_from_score() {
        assert_eq!(ContentLoadStrategy::from_score(0.9), ContentLoadStrategy::Full);
        assert_eq!(ContentLoadStrategy::from_score(0.86), ContentLoadStrategy::Full);
        assert_eq!(ContentLoadStrategy::from_score(0.7), ContentLoadStrategy::Overview);
        assert_eq!(ContentLoadStrategy::from_score(0.61), ContentLoadStrategy::Overview);
        assert_eq!(ContentLoadStrategy::from_score(0.5), ContentLoadStrategy::Abstract);
        assert_eq!(ContentLoadStrategy::from_score(0.0), ContentLoadStrategy::Abstract);
    }

    #[test]
    fn test_load_strategy_to_content_level() {
        assert_eq!(ContentLoadStrategy::Full.to_content_level(), ContentLevel::Detail);
        assert_eq!(ContentLoadStrategy::Overview.to_content_level(), ContentLevel::Overview);
        assert_eq!(ContentLoadStrategy::Abstract.to_content_level(), ContentLevel::Abstract);
    }
}
