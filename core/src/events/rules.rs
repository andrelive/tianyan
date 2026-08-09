//! 事件触发规则：事件匹配 → 动作。

use super::types::Event;

/// 规则动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventAction {
    /// 触发定时调度器中的任务（按任务 ID 或显示名匹配）。
    Task {
        /// 任务 ID 或显示名。
        name: String,
    },
    /// 唤醒会话 ID 以指定前缀开头的所有会话的主 agent。
    Wake {
        /// 会话 ID 前缀。
        session_prefix: String,
    },
}

/// 事件触发规则：pattern 匹配 → action。
///
/// 文本格式：`<pattern> => <action>`（见 [`parse_rules`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRule {
    /// 匹配模式：路径 glob（`*` 通配任意字符序列，含路径分隔符）或
    /// `webhook:<名称>`（精确匹配 webhook 名）。
    pub pattern: String,
    /// 命中后的动作。
    pub action: EventAction,
}

impl EventRule {
    /// 判断事件是否命中本规则。
    ///
    /// - 文件事件按路径 glob 匹配（`\` 与 `/` 视为等价，大小写敏感）；
    /// - webhook 事件精确匹配 `webhook:<名称>` 模式的名称部分。
    pub fn matches(&self, event: &Event) -> bool {
        match event {
            Event::Webhook { name, .. } => self
                .pattern
                .strip_prefix("webhook:")
                .is_some_and(|expected| expected == name),
            Event::FileCreated { path }
            | Event::FileModified { path }
            | Event::FileRemoved { path } => {
                if self.pattern.starts_with("webhook:") {
                    return false;
                }
                let path_str = path.to_string_lossy().replace('\\', "/");
                let pattern = normalize_pattern(&self.pattern.replace('\\', "/"));
                glob_match(&pattern, &path_str)
            }
        }
    }
}

/// 解析规则文本列表为规则集合。
///
/// 每项格式：`<pattern> => task:<任务名>` 或 `<pattern> => wake:<会话前缀>`；
/// pattern 为路径 glob 或 `webhook:<名称>`。
///
/// 空行、`#` 注释行与格式非法的行（缺少 `=>`、未知动作、空 pattern/动作名）
/// 被忽略并记录 warn 日志，不影响其余规则。
pub fn parse_rules(lines: &[String]) -> Vec<EventRule> {
    let mut rules = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((pattern, action_part)) = line.split_once("=>") else {
            tracing::warn!(line = %line, "事件规则格式非法（缺少 =>），已忽略");
            continue;
        };
        let pattern = pattern.trim();
        let action_part = action_part.trim();
        let action = if let Some(name) = action_part.strip_prefix("task:") {
            let name = name.trim();
            if name.is_empty() {
                tracing::warn!(line = %line, "事件规则格式非法（task 动作名为空），已忽略");
                continue;
            }
            EventAction::Task {
                name: name.to_string(),
            }
        } else if let Some(prefix) = action_part.strip_prefix("wake:") {
            let prefix = prefix.trim();
            if prefix.is_empty() {
                tracing::warn!(line = %line, "事件规则格式非法（wake 前缀为空），已忽略");
                continue;
            }
            EventAction::Wake {
                session_prefix: prefix.to_string(),
            }
        } else {
            tracing::warn!(line = %line, "事件规则格式非法（未知动作，应为 task: 或 wake:），已忽略");
            continue;
        };
        if pattern.is_empty() {
            tracing::warn!(line = %line, "事件规则格式非法（pattern 为空），已忽略");
            continue;
        }
        if pattern.starts_with("webhook:") && pattern.len() == "webhook:".len() {
            tracing::warn!(line = %line, "事件规则格式非法（webhook 名称为空），已忽略");
            continue;
        }
        rules.push(EventRule {
            pattern: pattern.to_string(),
            action,
        });
    }
    rules
}

/// 归一化 glob 模式，使 `**` 语义可用：
///
/// 1. 连续 `*` 折叠为单个 `*`；
/// 2. 丢弃紧跟 `*` 的 `/`（`dir/**/x` 与 `dir/*/x` 都能匹配 `dir/x` 与任意深层文件）。
///
/// 配合 [`glob_match`]（`*` 跨路径分隔符匹配）实现"任意层级目录"语义。
fn normalize_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut last_star = false;
    for c in pattern.chars() {
        if c == '*' {
            if last_star {
                continue; // 折叠连续 `*`
            }
            out.push('*');
            last_star = true;
        } else if c == '/' && last_star {
            // 丢弃紧跟 `*` 的 `/`（`*` 本身已跨路径分隔符匹配）
            continue;
        } else {
            out.push(c);
            last_star = false;
        }
    }
    out
}

/// 简单 glob 匹配：仅支持 `*`（匹配任意字符序列，含路径分隔符）。
///
/// 无 `*` 时退化为精确匹配；大小写敏感。使用经典两指针 + star 回溯算法，
/// 避免引入 glob 依赖。
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    // 最近一个 `*` 的位置与对应的回溯点
    let (mut star, mut mark) = (None, 0usize);
    while ti < text.len() {
        if pi < pattern.len() && pattern[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == text[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(star_pos) = star {
            // 回溯：`*` 多吞一个字符再试
            pi = star_pos + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    // 文本耗尽后，跳过模式末尾的连续 `*`
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_rules_valid_formats() {
        let lines = vec![
            "src/**/*.rs => task:fmt".to_string(),
            "docs/*.md => wake:session_work".to_string(),
            "webhook:deploy => wake:deploy_session".to_string(),
            "  notes.txt => task: sync ".to_string(),
        ];
        let rules = parse_rules(&lines);
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].pattern, "src/**/*.rs");
        assert_eq!(
            rules[0].action,
            EventAction::Task {
                name: "fmt".to_string()
            }
        );
        assert_eq!(
            rules[1].action,
            EventAction::Wake {
                session_prefix: "session_work".to_string()
            }
        );
        assert_eq!(rules[2].pattern, "webhook:deploy");
        assert_eq!(
            rules[3].action,
            EventAction::Task {
                name: "sync".to_string()
            },
            "多余空白应被裁剪"
        );
    }

    #[test]
    fn test_parse_rules_ignores_invalid_lines() {
        let lines = vec![
            String::new(),
            "   ".to_string(),
            "# 注释行".to_string(),
            "no-arrow".to_string(),
            "src => unknown:action".to_string(),
            "=> task:x".to_string(),
            "src => task:".to_string(),
            "src => wake:".to_string(),
            "webhook: => wake:session".to_string(),
            "src => task:fmt".to_string(),
        ];
        let rules = parse_rules(&lines);
        assert_eq!(rules.len(), 1, "非法行应被忽略，仅保留最后一条合法规则");
        assert_eq!(rules[0].pattern, "src");
        assert_eq!(
            rules[0].action,
            EventAction::Task {
                name: "fmt".to_string()
            }
        );
    }

    #[test]
    fn test_rule_matches_file_glob() {
        let rule = EventRule {
            pattern: "src/**/*.rs".to_string(),
            action: EventAction::Task {
                name: "fmt".to_string(),
            },
        };
        assert!(rule.matches(&Event::FileCreated {
            path: PathBuf::from("src/main.rs")
        }));
        assert!(rule.matches(&Event::FileCreated {
            path: PathBuf::from("src/sub/deep.rs")
        }));
        assert!(!rule.matches(&Event::FileCreated {
            path: PathBuf::from("docs/main.rs")
        }));
        assert!(!rule.matches(&Event::FileModified {
            path: PathBuf::from("src/main.txt")
        }));
    }

    #[test]
    fn test_rule_matches_webhook_name() {
        let rule = EventRule {
            pattern: "webhook:deploy".to_string(),
            action: EventAction::Wake {
                session_prefix: "d".to_string(),
            },
        };
        assert!(rule.matches(&Event::Webhook {
            name: "deploy".to_string(),
            payload: serde_json::json!({}),
        }));
        assert!(!rule.matches(&Event::Webhook {
            name: "deploy-prod".to_string(),
            payload: serde_json::json!({}),
        }));
        // 文件事件不匹配 webhook 模式
        assert!(!rule.matches(&Event::FileCreated {
            path: PathBuf::from("webhook:deploy")
        }));
        // 路径模式不匹配 webhook 事件
        let path_rule = EventRule {
            pattern: "*.rs".to_string(),
            action: EventAction::Task {
                name: "fmt".to_string(),
            },
        };
        assert!(!path_rule.matches(&Event::Webhook {
            name: "deploy".to_string(),
            payload: serde_json::json!({}),
        }));
    }

    #[test]
    fn test_rule_matches_windows_paths() {
        let rule = EventRule {
            pattern: r"src\*.rs".to_string(),
            action: EventAction::Task {
                name: "t".to_string(),
            },
        };
        assert!(rule.matches(&Event::FileModified {
            path: PathBuf::from(r"src\main.rs")
        }));
    }

    #[test]
    fn test_glob_match_basics() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "src/main.rs"), "`*` 应跨路径分隔符匹配");
        assert!(glob_match("docs/*", "docs/a.md"));
        assert!(glob_match("docs/*", "docs/sub/a.md"));
        assert!(glob_match("a*b*c", "aXbYc"));
        assert!(glob_match("exact", "exact"));
        assert!(glob_match("", ""));
        assert!(!glob_match("*.rs", "main.txt"));
        assert!(!glob_match("a*c", "ab"));
        assert!(!glob_match("exact", "exact2"));
        assert!(!glob_match("", "x"));
        assert!(!glob_match("a*", "b"));
    }

    #[test]
    fn test_normalize_pattern_supports_double_star() {
        // `src/**/*.rs` 归一化为 `src/*.rs`：直接子文件与任意深层都匹配
        let normalized = normalize_pattern("src/**/*.rs");
        assert_eq!(normalized, "src/*.rs");
        assert!(glob_match(&normalized, "src/main.rs"));
        assert!(glob_match(&normalized, "src/a/b/main.rs"));

        // 顶层 `**/*.md`
        let normalized = normalize_pattern("**/*.md");
        assert_eq!(normalized, "*.md");
        assert!(glob_match(&normalized, "docs/sub/readme.md"));
        assert!(glob_match(&normalized, "readme.md"));

        // `docs/**/x.md` 归一化为 `docs/*x.md`：零层或多层目录都匹配
        let normalized = normalize_pattern("docs/**/x.md");
        assert_eq!(normalized, "docs/*x.md");
        assert!(glob_match(&normalized, "docs/x.md"));
        assert!(glob_match(&normalized, "docs/a/b/x.md"));
        assert!(!glob_match(&normalized, "docs/y.md"));

        // 尾部 `**`
        let normalized = normalize_pattern("src/**");
        assert_eq!(normalized, "src/*");
        assert!(glob_match(&normalized, "src/main.rs"));

        // 无 `*` 的模式保持不变（精确匹配）
        assert_eq!(normalize_pattern("notes.txt"), "notes.txt");
    }
}
