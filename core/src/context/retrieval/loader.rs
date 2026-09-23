//! 内容加载模块。
//!
//! 基于相关性分数的内容加载策略（ADR-001 修订 2026-09-22：**L2 不再自动加载**）。

use crate::common::types::ContentLevel;

/// 基于相关性分数的内容加载策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentLoadStrategy {
    /// 加载概览（L1）——中等及以上相关性（分数 > 0.6）。
    ///
    /// L1 = 目录（叶节点）或全文直用（短内容）；加载时若为结构化目录，会前置
    /// L0 简介（"简介 + 目录"，见 `DualLayerRetriever::compose_overview`）。
    Overview,
    /// 仅加载摘要（L0）——低相关性（分数 <= 0.6）。
    Abstract,
}

impl ContentLoadStrategy {
    /// 根据相关性分数确定策略。
    ///
    /// ADR-001 修订（2026-09-22）：**L2 不再自动加载**——命中即"简介（+目录）"，
    /// 正文按章节 range 取段（`vfs_read` offset/limit）；短内容下 L1"直用" =
    /// 全文，行为等价于旧制。
    pub fn from_score(score: f32) -> Self {
        if score > 0.6 {
            Self::Overview
        } else {
            Self::Abstract
        }
    }

    /// 获取此策略对应的内容层级。
    pub fn to_content_level(self) -> ContentLevel {
        match self {
            Self::Overview => ContentLevel::Overview,
            Self::Abstract => ContentLevel::Abstract,
        }
    }
}

/// 组装 L1 的消费形态（纯函数，便于单测）：
/// - L1 为结构化目录（`parse_doc_index` 成功）→ "L0 简介 + 渲染目录"
///   （简介为空则只给目录）；
/// - 非目录（全文直用/旧概览）→ 原样返回（**不前置简介**，避免前缀膨胀
///   ——全文已含内容）。
pub(crate) fn compose_overview_parts(abstract_text: &str, l1: &str) -> String {
    let Some(index) = crate::vfs::parse_doc_index(l1) else {
        return l1.to_string();
    };
    let mut out = String::new();
    if !abstract_text.trim().is_empty() {
        out.push_str(abstract_text.trim());
        out.push_str("\n\n");
    }
    out.push_str(&crate::vfs::render_doc_index(&index));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_strategy_from_score() {
        // ADR-001 修订：L2 不再自动加载——>0.6 → L1（简介+目录）；否则 L0
        assert_eq!(
            ContentLoadStrategy::from_score(0.9),
            ContentLoadStrategy::Overview
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.86),
            ContentLoadStrategy::Overview
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.7),
            ContentLoadStrategy::Overview
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.61),
            ContentLoadStrategy::Overview
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.6),
            ContentLoadStrategy::Abstract
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.5),
            ContentLoadStrategy::Abstract
        );
        assert_eq!(
            ContentLoadStrategy::from_score(0.0),
            ContentLoadStrategy::Abstract
        );
    }

    #[test]
    fn test_load_strategy_to_content_level() {
        assert_eq!(
            ContentLoadStrategy::Overview.to_content_level(),
            ContentLevel::Overview
        );
        assert_eq!(
            ContentLoadStrategy::Abstract.to_content_level(),
            ContentLevel::Abstract
        );
    }

    #[test]
    fn test_compose_overview_parts_directory_prepends_abstract() {
        // 目录 → "简介 + 渲染目录"（markdown 列表，附行号供按需取段）
        let index = r#"{"kind":"index","sections":[{"title":"安装","summary":"如何安装","start_line":3,"end_line":10}]}"#;
        let out = compose_overview_parts("支付接入指南", index);
        assert!(out.starts_with("支付接入指南\n\n"), "简介应前置: {out}");
        assert!(
            out.contains("- 安装（行 3-10）：如何安装"),
            "目录应渲染为 markdown: {out}"
        );
    }

    #[test]
    fn test_compose_overview_parts_passthrough_and_empty_abstract() {
        // 非目录（全文直用/旧概览）→ 原样返回，不前置简介（避免前缀膨胀）
        let prose = "这是一段普通概览文本";
        assert_eq!(compose_overview_parts("简介", prose), prose);
        // 目录但简介为空 → 只给目录（无前导空行）
        let index = r#"{"kind":"index","sections":[{"title":"A","summary":"a"}]}"#;
        let out = compose_overview_parts("   ", index);
        assert!(out.starts_with("- A：a"), "{out}");
    }
}
