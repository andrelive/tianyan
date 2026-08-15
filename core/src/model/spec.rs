//! 模型上下文规格（ModelSpec）与内置规格表。
//!
//! 本模块定义模型的上下文窗口规格（总窗口、单次输出上限、单次嵌入输入上限），
//! 提供内置规格表（按 provider 前缀 + 模型名前缀匹配）与规格解析/钳制辅助函数。
//!
//! # 语义
//!
//! - `context_length`：总窗口（输入 + 输出共享）
//! - `max_output_tokens`：单次输出上限
//! - `max_input_tokens`：单次嵌入输入上限（chat 模型该字段 = context − output）
//!
//! 解析优先级：显式规格 > 内置规格表 > 全局默认值，返回前经过 [clamp_spec] 钳制。

use serde::{Deserialize, Serialize};

use crate::common::token_estimator::TokenEstimator;
use crate::model::types::ThinkingEffort;

/// 模型上下文规格。
///
/// 描述一个模型的上下文窗口预算：
///
/// - `context_length`：总窗口大小（输入 + 输出共享）
/// - `max_output_tokens`：单次输出 token 上限
/// - `max_input_tokens`：单次嵌入输入上限（chat 模型 = context − output）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// 总窗口大小（输入 + 输出共享）。
    pub context_length: usize,
    /// 单次输出 token 上限。
    pub max_output_tokens: usize,
    /// 单次嵌入输入上限（chat 模型该字段 = context − output）。
    pub max_input_tokens: usize,
}

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            context_length: 32_768,
            max_output_tokens: 8_192,
            max_input_tokens: 8_192,
        }
    }
}

/// 内置规格条目（provider 前缀 + 模型名前缀）。
pub struct BuiltinEntry {
    /// provider 名称前缀（匹配忽略大小写）。
    pub provider_prefix: &'static str,
    /// 模型名称前缀（匹配忽略大小写）。
    pub model_prefix: &'static str,
    /// 上下文规格。
    pub spec: ModelSpec,
    /// 该模型支持的思考强度档位（每个模型自己的档位集；None 表示不支持思考）。
    pub reasoning_efforts: Option<&'static [ThinkingEffort]>,
}

/// 内置规格表。
///
/// 按 provider 前缀 + 模型名前缀匹配（均为 `eq_ignore_ascii_case`）。
/// deepseek-v4-flash 使用完整模型名精确匹配，其余使用短前缀。
pub static BUILTIN_SPECS: &[BuiltinEntry] = &[
    BuiltinEntry {
        provider_prefix: "deepseek",
        model_prefix: "deepseek-v4-flash",
        spec: ModelSpec {
            context_length: 1_000_000,
            max_output_tokens: 32_000,
            max_input_tokens: 968_000,
        },
        reasoning_efforts: Some(&[
            ThinkingEffort::Off,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ]),
    },
    BuiltinEntry {
        provider_prefix: "deepseek",
        model_prefix: "deepseek-r1",
        spec: ModelSpec {
            context_length: 128_000,
            max_output_tokens: 32_000,
            max_input_tokens: 96_000,
        },
        reasoning_efforts: Some(&[
            ThinkingEffort::Off,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ]),
    },
    BuiltinEntry {
        provider_prefix: "openai",
        model_prefix: "gpt-4o",
        spec: ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        },
        reasoning_efforts: None,
    },
    BuiltinEntry {
        provider_prefix: "anthropic",
        model_prefix: "claude",
        spec: ModelSpec {
            context_length: 200_000,
            max_output_tokens: 8_000,
            max_input_tokens: 192_000,
        },
        reasoning_efforts: None,
    },
    // qwen3 思考族须排在 qwen 通用条目之前（长前缀优先命中，qwen2.x 仍命中 qwen）
    BuiltinEntry {
        provider_prefix: "qwen",
        model_prefix: "qwen3",
        spec: ModelSpec {
            context_length: 131_000,
            max_output_tokens: 32_000,
            max_input_tokens: 99_000,
        },
        reasoning_efforts: Some(&[
            ThinkingEffort::Off,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ]),
    },
    BuiltinEntry {
        provider_prefix: "qwen",
        model_prefix: "qwen",
        spec: ModelSpec {
            context_length: 131_000,
            max_output_tokens: 8_000,
            max_input_tokens: 123_000,
        },
        reasoning_efforts: None,
    },
    BuiltinEntry {
        provider_prefix: "llama",
        model_prefix: "llama",
        spec: ModelSpec {
            context_length: 128_000,
            max_output_tokens: 8_000,
            max_input_tokens: 120_000,
        },
        reasoning_efforts: None,
    },
];

/// 在内置规格表中查找规格。
///
/// 三级匹配（按序，均为 `eq_ignore_ascii_case` 前缀比较）：
///
/// 1. provider 前缀匹配 + 模型名精确匹配（如 deepseek-v4-flash 用完整模型名）
/// 2. provider 前缀匹配 + 模型名前缀匹配（如 qwen2.5-72b-instruct 命中 "qwen" 前缀）
/// 3. 兜底：provider 前缀不匹配时，仅按模型名前缀匹配。模型名本身具强标识性，
///    网关 provider（如 "opencode"）挂载知名模型（如 "deepseek-v4-flash"）时也能命中。
pub fn builtin_spec(provider: &str, model: &str) -> Option<ModelSpec> {
    // 第一级：provider 前缀匹配 + 模型名精确匹配（如 deepseek-v4-flash 用完整模型名）
    for entry in BUILTIN_SPECS {
        if provider.eq_ignore_ascii_case(entry.provider_prefix)
            && model.eq_ignore_ascii_case(entry.model_prefix)
        {
            return Some(entry.spec);
        }
    }
    // 第二级：provider 前缀匹配 + 模型名前缀匹配（如 qwen2.5-72b-instruct 命中 "qwen" 前缀）
    for entry in BUILTIN_SPECS {
        if provider.eq_ignore_ascii_case(entry.provider_prefix)
            && model
                .get(..entry.model_prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(entry.model_prefix))
        {
            return Some(entry.spec);
        }
    }
    // 第三级（兜底）：provider 前缀不匹配时，仅按模型名前缀匹配。
    // 模型名本身具强标识性：deepseek-v4-flash 挂在网关 provider "opencode" 下也能命中。
    for entry in BUILTIN_SPECS {
        if model
            .get(..entry.model_prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(entry.model_prefix))
        {
            return Some(entry.spec);
        }
    }
    None
}

/// 在内置表中查找模型声明的思考强度档位（三级匹配与 [`builtin_spec`] 相同）。
///
/// 每个模型自己的档位集：返回 None 表示该模型不支持思考（前端不显示思考选择）。
pub fn builtin_reasoning_efforts(provider: &str, model: &str) -> Option<&'static [ThinkingEffort]> {
    for entry in BUILTIN_SPECS {
        if provider.eq_ignore_ascii_case(entry.provider_prefix)
            && model.eq_ignore_ascii_case(entry.model_prefix)
        {
            return entry.reasoning_efforts;
        }
    }
    for entry in BUILTIN_SPECS {
        if provider.eq_ignore_ascii_case(entry.provider_prefix)
            && model
                .get(..entry.model_prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(entry.model_prefix))
        {
            return entry.reasoning_efforts;
        }
    }
    for entry in BUILTIN_SPECS {
        if model
            .get(..entry.model_prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(entry.model_prefix))
        {
            return entry.reasoning_efforts;
        }
    }
    None
}

/// 解析最终规格。
///
/// 优先级：显式规格 > 内置规格表 > 全局默认值；返回前经过 [clamp_spec] 钳制。
pub fn resolve_spec(explicit: Option<ModelSpec>, provider: &str, model: &str) -> ModelSpec {
    let base = explicit
        .or_else(|| builtin_spec(provider, model))
        .unwrap_or_default();
    clamp_spec(base)
}

/// 钳制规格，保证输入预算非负且非零。
///
/// 当 `context_length <= max_output_tokens` 时把 `context_length` 抬到
/// `max_output_tokens + 1`（防止输入预算为负）；`max_input_tokens` 为 0 时置 8192。
pub fn clamp_spec(spec: ModelSpec) -> ModelSpec {
    let context_length = if spec.context_length <= spec.max_output_tokens {
        spec.max_output_tokens + 1
    } else {
        spec.context_length
    };
    let max_input_tokens = if spec.max_input_tokens == 0 {
        8_192
    } else {
        spec.max_input_tokens
    };
    ModelSpec {
        context_length,
        max_output_tokens: spec.max_output_tokens,
        max_input_tokens,
    }
}

/// 由可选字段合成规格。
///
/// 三字段全为 `None` 时返回 `None`；否则返回 `Some(ModelSpec)`，
/// 各 `None` 字段使用默认值（context 32768、output 8192、input 8192）。
pub fn from_entry_fields(
    context_length: Option<usize>,
    max_output_tokens: Option<usize>,
    max_input_tokens: Option<usize>,
) -> Option<ModelSpec> {
    if context_length.is_none() && max_output_tokens.is_none() && max_input_tokens.is_none() {
        return None;
    }
    let defaults = ModelSpec::default();
    Some(ModelSpec {
        context_length: context_length.unwrap_or(defaults.context_length),
        max_output_tokens: max_output_tokens.unwrap_or(defaults.max_output_tokens),
        max_input_tokens: max_input_tokens.unwrap_or(defaults.max_input_tokens),
    })
}

/// 返回不大于 `v` 的最大 2 的幂；`v = 0` 时返回 0。
pub fn pow2_floor(v: usize) -> usize {
    if v == 0 {
        return 0;
    }
    1usize << (usize::BITS - 1 - v.leading_zeros())
}

/// 输出预算下限：低于该值视为窗口余量不足，返回 `None` 交由压缩链路恢复。
const MIN_MAX_TOKENS: usize = 1024;

/// 输出预算保留系数（×9/10），为输入估算误差与输出增长留出余量。
const MAX_TOKENS_SAFETY: usize = 9;

/// 计算本次请求的动态 max_tokens。
///
/// input_est 优先使用上次请求实测输入（usage.prompt_tokens），
/// 无实测时退化为对新增消息的 TokenEstimator 估算；
/// 输出预算 = min(spec.max_output_tokens, pow2_floor((context − input_est) × 0.9))；
/// 结果 < 1024 时返回 None（调用方跳过请求，交由压缩链路恢复）。
pub fn dynamic_max_tokens(
    spec: &ModelSpec,
    measured_input: Option<usize>,
    new_messages: &[crate::common::types::Message],
) -> Option<usize> {
    let input_est =
        measured_input.unwrap_or_else(|| TokenEstimator::new().estimate_messages(new_messages));
    let remaining = spec.context_length.saturating_sub(input_est);
    let budget = pow2_floor(remaining.saturating_mul(MAX_TOKENS_SAFETY) / 10);
    let max_tokens = spec.max_output_tokens.min(budget);
    (max_tokens >= MIN_MAX_TOKENS).then_some(max_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_spec_values() {
        let d = ModelSpec::default();
        assert_eq!(d.context_length, 32_768);
        assert_eq!(d.max_output_tokens, 8_192);
        assert_eq!(d.max_input_tokens, 8_192);
    }

    #[test]
    fn test_builtin_deepseek_v4_flash_exact() {
        let spec = builtin_spec("deepseek", "deepseek-v4-flash").unwrap();
        assert_eq!(spec.context_length, 1_000_000);
        assert_eq!(spec.max_output_tokens, 32_000);
        assert_eq!(spec.max_input_tokens, 968_000);
    }

    #[test]
    fn test_builtin_openai_gpt4o_prefix() {
        let spec = builtin_spec("openai", "gpt-4o-mini").unwrap();
        assert_eq!(spec.context_length, 128_000);
        assert_eq!(spec.max_output_tokens, 16_000);
        assert_eq!(spec.max_input_tokens, 112_000);
    }

    #[test]
    fn test_builtin_anthropic_claude_prefix() {
        let spec = builtin_spec("anthropic", "claude-3-5-sonnet-20241022").unwrap();
        assert_eq!(spec.context_length, 200_000);
        assert_eq!(spec.max_output_tokens, 8_000);
        assert_eq!(spec.max_input_tokens, 192_000);
    }

    #[test]
    fn test_builtin_qwen_prefix() {
        let spec = builtin_spec("qwen", "qwen2.5-72b-instruct").unwrap();
        assert_eq!(spec.context_length, 131_000);
        assert_eq!(spec.max_output_tokens, 8_000);
        assert_eq!(spec.max_input_tokens, 123_000);
    }

    #[test]
    fn test_builtin_llama_prefix() {
        let spec = builtin_spec("llama", "llama-3.1-70b-instruct").unwrap();
        assert_eq!(spec.context_length, 128_000);
        assert_eq!(spec.max_output_tokens, 8_000);
        assert_eq!(spec.max_input_tokens, 120_000);
    }

    #[test]
    fn test_builtin_case_insensitive() {
        let spec = builtin_spec("DeepSeek", "DeepSeek-V4-Flash").unwrap();
        assert_eq!(spec.context_length, 1_000_000);
        let spec2 = builtin_spec("OPENAI", "GPT-4O-TURBO").unwrap();
        assert_eq!(spec2.context_length, 128_000);
        let spec3 = builtin_spec("Anthropic", "CLAUDE-3-5").unwrap();
        assert_eq!(spec3.context_length, 200_000);
    }

    #[test]
    fn test_builtin_unknown_model_none() {
        assert!(builtin_spec("deepseek", "deepseek-chat").is_none());
        assert!(builtin_spec("openai", "gpt-3.5-turbo").is_none());
        assert!(builtin_spec("unknown-provider", "totally-unknown-model").is_none());
    }

    #[test]
    fn test_builtin_fallback_model_name_only_gateway_provider() {
        // 网关 provider "opencode" 挂载 deepseek-v4-flash：provider 前缀不匹配，
        // 第三级兜底仅按模型名前缀命中 deepseek 条目
        let spec = builtin_spec("opencode", "deepseek-v4-flash").unwrap();
        assert_eq!(spec.context_length, 1_000_000);
        assert_eq!(spec.max_output_tokens, 32_000);
        assert_eq!(spec.max_input_tokens, 968_000);
    }

    #[test]
    fn test_builtin_fallback_model_name_only_custom_provider() {
        // 自定义 provider + gpt-4o 前缀模型 → 命中 openai 条目
        let spec = builtin_spec("custom", "gpt-4o-mini").unwrap();
        assert_eq!(spec.context_length, 128_000);
        assert_eq!(spec.max_output_tokens, 16_000);
        assert_eq!(spec.max_input_tokens, 112_000);
    }

    #[test]
    fn test_builtin_fallback_model_name_only_ollama_provider() {
        // ollama + "llama3.1:8b"：get(..5) 取 "llama" 前缀，冒号不影响边界
        let spec = builtin_spec("ollama", "llama3.1:8b").unwrap();
        assert_eq!(spec.context_length, 128_000);
        assert_eq!(spec.max_output_tokens, 8_000);
        assert_eq!(spec.max_input_tokens, 120_000);
    }

    #[test]
    fn test_builtin_fallback_still_none_for_unknown_model() {
        // 第三级也不匹配 → None，避免误伤
        assert!(builtin_spec("unknown", "totally-unknown-model").is_none());
    }

    #[test]
    fn test_resolve_explicit_wins() {
        let explicit = ModelSpec {
            context_length: 64_000,
            max_output_tokens: 4_000,
            max_input_tokens: 60_000,
        };
        let got = resolve_spec(Some(explicit), "openai", "gpt-4o");
        assert_eq!(got, explicit);
    }

    #[test]
    fn test_resolve_builtin_when_no_explicit() {
        let got = resolve_spec(None, "qwen", "qwen2.5-72b-instruct");
        assert_eq!(got.context_length, 131_000);
        assert_eq!(got.max_output_tokens, 8_000);
        assert_eq!(got.max_input_tokens, 123_000);
    }

    #[test]
    fn test_resolve_default_when_unknown() {
        let got = resolve_spec(None, "unknown-provider", "some-model");
        assert_eq!(got, ModelSpec::default());
    }

    #[test]
    fn test_clamp_context_below_output() {
        let spec = ModelSpec {
            context_length: 1_000,
            max_output_tokens: 2_000,
            max_input_tokens: 0,
        };
        let clamped = clamp_spec(spec);
        assert_eq!(clamped.context_length, 2_001);
        assert_eq!(clamped.max_input_tokens, 8_192);
    }

    #[test]
    fn test_clamp_zero_input_budget() {
        let spec = ModelSpec {
            context_length: 32_768,
            max_output_tokens: 8_192,
            max_input_tokens: 0,
        };
        let clamped = clamp_spec(spec);
        assert_eq!(clamped.context_length, 32_768);
        assert_eq!(clamped.max_input_tokens, 8_192);
    }

    #[test]
    fn test_clamp_healthy_spec_unchanged() {
        let spec = ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        };
        assert_eq!(clamp_spec(spec), spec);
    }

    #[test]
    fn test_from_entry_fields_all_none() {
        assert_eq!(from_entry_fields(None, None, None), None);
    }

    #[test]
    fn test_from_entry_fields_partial_some() {
        let got = from_entry_fields(Some(64_000), None, None).unwrap();
        assert_eq!(got.context_length, 64_000);
        assert_eq!(got.max_output_tokens, 8_192);
        assert_eq!(got.max_input_tokens, 8_192);

        let got2 = from_entry_fields(None, Some(4_000), Some(60_000)).unwrap();
        assert_eq!(got2.context_length, 32_768);
        assert_eq!(got2.max_output_tokens, 4_000);
        assert_eq!(got2.max_input_tokens, 60_000);
    }

    #[test]
    fn test_pow2_floor() {
        assert_eq!(pow2_floor(0), 0);
        assert_eq!(pow2_floor(1), 1);
        assert_eq!(pow2_floor(2), 2);
        assert_eq!(pow2_floor(3), 2);
        assert_eq!(pow2_floor(1000), 512);
        assert_eq!(pow2_floor(1 << 18), 1 << 18);
    }

    #[test]
    fn test_dynamic_max_tokens_measured_input_priority() {
        // 实测优先：remaining = 1_000_000 − 700_000 = 300_000，
        // ×0.9 = 270_000 → pow2_floor = 262_144，min(32_000, 262_144) = 32_000（cap 生效）
        let spec = ModelSpec {
            context_length: 1_000_000,
            max_output_tokens: 32_000,
            max_input_tokens: 968_000,
        };
        let messages = vec![crate::common::types::Message::user("unused payload")];
        assert_eq!(
            dynamic_max_tokens(&spec, Some(700_000), &messages),
            Some(32_000)
        );
    }

    #[test]
    fn test_dynamic_max_tokens_estimate_fallback() {
        // 无实测走估算：新消息极小 → 预算巨大 → 返回 max_output_tokens
        let spec = ModelSpec {
            context_length: 1_000_000,
            max_output_tokens: 32_000,
            max_input_tokens: 968_000,
        };
        let messages = vec![crate::common::types::Message::user("hi")];
        assert_eq!(dynamic_max_tokens(&spec, None, &messages), Some(32_000));
    }

    #[test]
    fn test_dynamic_max_tokens_near_window() {
        // remaining 28_000 × 0.9 = 25_200 → pow2_floor = 16_384 → min(16_000, 16_384) = 16_000
        let spec = ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        };
        let messages = vec![];
        assert_eq!(
            dynamic_max_tokens(&spec, Some(100_000), &messages),
            Some(16_000)
        );
    }

    #[test]
    fn test_dynamic_max_tokens_larger_input() {
        // remaining 8_000 × 0.9 = 7_200 → pow2_floor = 4_096 → min(16_000, 4_096) = 4_096
        let spec = ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        };
        let messages = vec![];
        assert_eq!(
            dynamic_max_tokens(&spec, Some(120_000), &messages),
            Some(4_096)
        );
    }

    #[test]
    fn test_dynamic_max_tokens_over_window() {
        // 输入超窗：saturating_sub → remaining 0 → 预算 0 < 1024 → None
        let spec = ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        };
        let messages = vec![];
        assert_eq!(dynamic_max_tokens(&spec, Some(200_000), &messages), None);
    }

    #[test]
    fn test_dynamic_max_tokens_below_min_threshold() {
        // remaining 1137 × 0.9 = 1023 → pow2_floor = 512 < 1024 → None
        let spec = ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        };
        let messages = vec![];
        assert_eq!(
            dynamic_max_tokens(&spec, Some(128_000 - 1137), &messages),
            None
        );
    }

    #[test]
    fn test_dynamic_max_tokens_at_min_threshold() {
        // remaining 1138 × 0.9 = 1024 → pow2_floor = 1024 → Some(1024)
        let spec = ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        };
        let messages = vec![];
        assert_eq!(
            dynamic_max_tokens(&spec, Some(128_000 - 1138), &messages),
            Some(1024)
        );
    }

    #[test]
    fn test_dynamic_max_tokens_cap_wins() {
        // 预算 65_536 > cap 16_000 → 返回 cap
        let spec = ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        };
        let messages = vec![];
        assert_eq!(dynamic_max_tokens(&spec, Some(0), &messages), Some(16_000));
    }
}
