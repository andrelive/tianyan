//! Provider 请求重试策略（指数退避 + 最大重试次数）。
//!
//! 对齐 opencode / DSH dsh-llm-retry：默认策略内置，对全部 provider 生效；
//! 请求级临时失败（HTTP 429 / 5xx / 网络 / 超时）自动退避重试，
//! 每个 provider 可选覆盖策略（缺省用全局默认）。
//!
//! 错误分类契约只在 provider 请求层判定（HTTP 状态 / 传输错误），
//! 不新增 TianyanError 变体（ADR-014 语义谓词）——重试是请求层的自我保护。

use serde::{Deserialize, Serialize};

/// 全局内置默认最大重试次数（首次请求不计入；对齐 DSH DEFAULT_MAX_RETRIES=2）。
pub const DEFAULT_MAX_RETRIES: u32 = 2;
/// 全局内置默认首次退避延迟（毫秒）。
pub const DEFAULT_INITIAL_DELAY_MS: u64 = 500;
/// 全局内置默认退避延迟上限（毫秒）。
pub const DEFAULT_MAX_DELAY_MS: u64 = 10_000;
/// 全局内置默认抖动比例 [0,1]。
pub const DEFAULT_JITTER_RATIO: f64 = 0.1;

/// 单次 provider 请求的重试策略（每 provider 可选覆盖；缺省用全局内置默认）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct RetryPolicy {
    /// 最大重试次数（首次请求不计入；0 = 不重试）。
    pub max_retries: u32,
    /// 首次退避延迟（毫秒）。
    pub initial_delay_ms: u64,
    /// 退避延迟上限（毫秒）。
    pub max_delay_ms: u64,
    /// 抖动比例 [0,1]（退避乘以 jitter = 1-r + 2r*rand，与 DSH 一致）。
    pub jitter_ratio: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            initial_delay_ms: DEFAULT_INITIAL_DELAY_MS,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
            jitter_ratio: DEFAULT_JITTER_RATIO,
        }
    }
}

impl RetryPolicy {
    /// 校验策略合法性（供配置 validate 复用；返回错误原因）。
    pub fn validate(&self) -> Result<(), String> {
        if self.max_retries > 20 {
            return Err("max_retries 不能超过 20".to_string());
        }
        if self.initial_delay_ms == 0 {
            return Err("initial_delay_ms 必须大于 0".to_string());
        }
        if self.max_delay_ms == 0 || self.max_delay_ms < self.initial_delay_ms {
            return Err("max_delay_ms 必须大于 0 且不小于 initial_delay_ms".to_string());
        }
        if !(0.0..=1.0).contains(&self.jitter_ratio) {
            return Err("jitter_ratio 必须在 0 到 1 之间".to_string());
        }
        Ok(())
    }

    /// 第 `retry` 次重试（1-based）前的退避延迟（毫秒）。
    ///
    /// 指数退避：initial * 2^(retry-1)，封顶 max_delay_ms，
    /// 再乘抖动 (1-jitter_ratio) + 2*jitter_ratio*r（确定性伪随机，无需依赖）。
    pub fn delay_ms(&self, retry: u32) -> u64 {
        let exp = (retry as u64).saturating_sub(1).min(10);
        let base = self
            .initial_delay_ms
            .saturating_mul(1u64 << exp)
            .min(self.max_delay_ms);
        let r = pseudo_random();
        let jitter = (1.0 - self.jitter_ratio) + 2.0 * self.jitter_ratio * r;
        let ms = ((base as f64) * jitter) as u64;
        ms.min(self.max_delay_ms)
    }
}

/// 轻量伪随机 [0,1)：基于系统时钟纳秒，避免为重试抖动引入随机数依赖。
fn pseudo_random() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let x = (nanos % 1_000_003) as f64 / 1_000_003.0;
    x.fract()
}

/// HTTP 状态码是否可重试：429（限流）与 5xx（服务端临时错误）。
pub fn should_retry_http(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// reqwest 传输层错误是否可重试：超时 / 连接失败。
/// 取消（客户端 abort）不重试——调用方应透传用户主动取消。
pub fn should_retry_transport(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect()
}

/// 按策略执行可重试调用：失败且可重试时退避后重试，直到成功、耗尽次数或不可重试。
///
/// - try_retryable：判定一次失败的类别（是否临时可重试）。
/// - call：执行单次尝试（返回 Result）。
pub async fn with_retry<V, E, C, F>(
    policy: &RetryPolicy,
    mut try_retryable: impl FnMut(&E) -> bool,
    mut call: C,
) -> Result<V, E>
where
    F: std::future::Future<Output = Result<V, E>>,
    C: FnMut() -> F,
{
    let mut retry = 0u32;
    loop {
        match call().await {
            Ok(value) => return Ok(value),
            Err(failure) => {
                if retry >= policy.max_retries || !try_retryable(&failure) {
                    return Err(failure);
                }
                let delay = policy.delay_ms(retry + 1);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                retry += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_values() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.initial_delay_ms, 500);
        assert_eq!(p.max_delay_ms, 10_000);
        assert_eq!(p.jitter_ratio, 0.1);
    }

    #[test]
    fn test_validate_policies() {
        assert!(RetryPolicy::default().validate().is_ok());
        let mut p = RetryPolicy::default();
        p.max_retries = 21;
        assert!(p.validate().is_err());
        let mut p = RetryPolicy::default();
        p.initial_delay_ms = 0;
        assert!(p.validate().is_err());
        let mut p = RetryPolicy::default();
        p.max_delay_ms = p.initial_delay_ms - 1;
        assert!(p.validate().is_err());
        let mut p = RetryPolicy::default();
        p.jitter_ratio = 1.5;
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_delay_capped_and_increases() {
        // jitter 0 → 精确度可控：retry=1 时 initial，之后翻倍，封顶 max
        let p = RetryPolicy {
            max_retries: 5,
            initial_delay_ms: 100,
            max_delay_ms: 10_000,
            jitter_ratio: 0.0,
        };
        assert_eq!(p.delay_ms(1), 100);
        assert_eq!(p.delay_ms(2), 200);
        assert_eq!(p.delay_ms(3), 400);
        // 封顶：2^7*100=12800 > 10000 → 10000
        assert_eq!(p.delay_ms(8), 10_000);
        assert_eq!(p.delay_ms(999), 10_000);
        // jitter > 0 时结果在 [base*(1-r), base*(1+r)] 内
        let d = RetryPolicy::default();
        let v = d.delay_ms(1);
        assert!(v >= 450 && v <= 550, "delay={v}");
        assert!(d.delay_ms(20) <= d.max_delay_ms);
    }

    #[test]
    fn test_should_retry_http_codes() {
        use reqwest::StatusCode;
        assert!(should_retry_http(StatusCode::TOO_MANY_REQUESTS));
        assert!(should_retry_http(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(should_retry_http(StatusCode::BAD_GATEWAY));
        assert!(!should_retry_http(StatusCode::BAD_REQUEST));
        assert!(!should_retry_http(StatusCode::UNAUTHORIZED));
        assert!(!should_retry_http(StatusCode::FORBIDDEN));
        assert!(!should_retry_http(StatusCode::OK));
    }

    #[tokio::test]
    async fn test_with_retry_succeeds_after_retry() {
        let policy = RetryPolicy {
            max_retries: 1,
            initial_delay_ms: 1,
            max_delay_ms: 2,
            jitter_ratio: 0.0,
        };
        let mut calls = 0u32;
        let result = with_retry(
            &policy,
            |e: &String| e == "temp",
            || {
                calls += 1;
                async move {
                    if calls == 1 {
                        Err("temp".to_string())
                    } else {
                        Ok(42u32)
                    }
                }
            },
        )
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn test_with_retry_exhausts() {
        let policy = RetryPolicy {
            max_retries: 1,
            initial_delay_ms: 1,
            max_delay_ms: 2,
            jitter_ratio: 0.0,
        };
        let mut calls = 0u32;
        let result = with_retry(
            &policy,
            |_| true,
            || {
                calls += 1;
                async move { Err::<u32, String>("boom".to_string()) }
            },
        )
        .await;
        assert_eq!(result, Err("boom".to_string()));
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn test_with_retry_non_retryable_stops() {
        let policy = RetryPolicy {
            max_retries: 5,
            initial_delay_ms: 1,
            max_delay_ms: 2,
            jitter_ratio: 0.0,
        };
        let mut calls = 0u32;
        let result = with_retry(
            &policy,
            |e: &String| e != "hard",
            || {
                calls += 1;
                async move { Err::<u32, String>("hard".to_string()) }
            },
        )
        .await;
        assert_eq!(result, Err("hard".to_string()));
        // 不可重试 → 只调用 1 次
        assert_eq!(calls, 1);
    }
}
