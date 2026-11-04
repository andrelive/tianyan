//! 调度规格（`Schedule`）× 错过策略（`MissedPolicy`）——ADR-024 之后的第二维（2026-11）。
//!
//! ## 为什么要把「何时该跑」和「错过了怎么办」拆开
//!
//! 旧模型只有一个 `interval_secs`：`now - last_run >= interval` 即到期。它把两件
//! 本质不同的事挤进一个数：
//!
//! 1. **何时该跑**（时间语义）：间隔？每天 15:30？每周一 09:00？
//! 2. **错过了怎么办**（补偿语义）：补跑？跳过？窗口内才补？
//!
//! 后果是真实踩到的：一个「收盘后 15:30 复盘」的任务，因为间隔制 + 宕机补跑，
//! 服务一重启就立刻补一次 —— 于是**在盘中执行**，业务上不仅无效而且有害。只恢复
//! cron 也不够：cron 只回答「何时该跑」，补跑策略照旧由 `last_run + interval` 决定。
//!
//! 因此拆成两个正交维度：
//!
//! - [`Schedule`]：时刻语义（间隔 / 每天 / 每周 / 一次性）；
//! - [`MissedPolicy`]：错过时的行为（立即补 / 跳过 / 窗口内补）。
//!
//! ## 游标与执行时间是两件事
//!
//! 判定「这个时刻点处理过没有」必须用一个**调度游标**（`last_fire_at`，指向最近
//! 处理过的**时刻点**），而不是「上次执行完成时间」。跳过也要推进游标——否则服务
//! 每晚关机、白天开机，会反复重判同一个已跳过的时刻点（补跑风暴）。
//!
//! ## 时区
//!
//! 一律按**本机本地时区**的墙钟计算（个人 PC 场景时区固定）：`Daily { at: "15:30" }`
//! 就是"本地时间每天 15:30"。ADR-024 反对 cron 的理由之一是「UTC 语义让本地触发时刻
//! 漂移」——这里明确以本地墙钟为准，规避了同一个坑。夏令时跳变导致本地时刻不存在
//! （春季前进那一个小时）时，该次不触发（`most_recent_occurrence` 返回 `None`）。

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveTime, TimeZone};

/// 调度规格：什么时候该跑。
///
/// 序列化形状面向 `scheduled_agent_tasks.json` 与前端（`tag = "kind"`）：
///
/// ```json
/// { "kind": "every",  "secs": 1800 }
/// { "kind": "daily",  "at": "15:30" }
/// { "kind": "weekly", "weekdays": [1, 3, 5], "at": "09:00" }
/// { "kind": "once",   "at": "2026-11-05T15:30" }
/// ```
///
/// `weekdays` 用 ISO 编号（1 = 周一 … 7 = 周日），与 `chrono::Weekday::number_from_monday`
/// 一致；`at` 是**本地墙钟**（`HH:MM`；`once` 用 `YYYY-MM-DDTHH:MM`）。
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// 间隔制：距上次执行 ≥ `secs` 秒（旧 `interval_secs` 的等价形态）。
    Every {
        /// 间隔秒数（最小 1；上层按业务给下限，如 agent 任务 60）。
        secs: u64,
    },
    /// 每天本地时间 `at`（`HH:MM`）。
    Daily {
        /// 本地墙钟时刻，`HH:MM`。
        at: String,
    },
    /// 每周指定几天的本地时间 `at`。
    Weekly {
        /// ISO 星期（1 = 周一 … 7 = 周日；去重排序后使用）。
        weekdays: Vec<u8>,
        /// 本地墙钟时刻，`HH:MM`。
        at: String,
    },
    /// 一次性定点（本地时间 `YYYY-MM-DDTHH:MM`）；执行后由上层停用。
    Once {
        /// 本地日期时间，`YYYY-MM-DDTHH:MM`（也接受带秒）。
        at: String,
    },
}

/// 错过策略：时刻点已过、但还没处理时怎么办。
///
/// 「准点」的定义带**扫描粒度容差**：调度器每 tick（默认 60s）扫一次，时刻点
/// 15:30:00 不太可能在 15:30:00 那一刻被扫到，因此 [`Self::Skip`] 允许
/// `delay <= tick` 内触发（见 [`decide_fire`]）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MissedPolicy {
    /// 立刻补跑一次（**旧行为**；维护类任务要这个：开机就把摘要/GC 补上）。
    #[serde(alias = "runonce")]
    RunOnce,
    /// 只在准点（含一个 tick 容差）触发，错过就等下一个自然时刻。
    Skip,
    /// 只在「时刻点 + `grace_secs`」窗口内补跑，超出就跳过。
    Within {
        /// 允许补跑的窗口长度（秒）。
        grace_secs: u64,
    },
}

impl Default for MissedPolicy {
    /// 默认沿用旧语义（ADR-024 的「宕机补跑一次」）——迁移时不改变既有任务行为。
    fn default() -> Self {
        Self::RunOnce
    }
}

/// 时刻点判定结果（[`decide_fire`] 的输出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireDecision {
    /// 该跑：`delay` = 距时刻点已过的秒数（0 = 准点）。
    Fire {
        /// 距时刻点已过的秒数。
        delay: u64,
    },
    /// 不跑（已错过窗口），但仍要推进游标，避免下次重复判定。
    Skip {
        /// 距时刻点已过的秒数（说明错过了多久）。
        delay: u64,
    },
    /// 该时刻点尚未到（或已被游标覆盖），不处理。
    NotDue,
}

/// 判定某个时刻点是否该触发。
///
/// `delay` = `now - anchor`（秒，≥0）。`tick_secs` 只用于 [`MissedPolicy::Skip`] 的
/// 准点容差——扫描粒度决定「准点」的宽度，写死 0 会让 `Skip` 永远不触发。
pub fn decide_fire(missed: MissedPolicy, delay: u64, tick_secs: u64) -> FireDecision {
    match missed {
        MissedPolicy::RunOnce => FireDecision::Fire { delay },
        MissedPolicy::Skip => {
            if delay <= tick_secs {
                FireDecision::Fire { delay }
            } else {
                FireDecision::Skip { delay }
            }
        }
        MissedPolicy::Within { grace_secs } => {
            if delay <= grace_secs {
                FireDecision::Fire { delay }
            } else {
                FireDecision::Skip { delay }
            }
        }
    }
}

impl Schedule {
    /// 间隔制的秒数（非间隔形态返回 `None`）。
    pub fn as_interval_secs(&self) -> Option<u64> {
        match self {
            Schedule::Every { secs } => Some(*secs),
            _ => None,
        }
    }

    /// 等价的「间隔秒数」——用于**落盘双写**（旧版本读新文件时仍能跑）。
    ///
    /// 旧版本只有 `interval_secs`，看不到 `schedule`。双写一个等价间隔让它「大致
    /// 正确」而不是瘫痪：
    /// - `Every` → 原值；
    /// - `Daily` → 24h；
    /// - `Weekly` → 一周 / 选中天数（单天 = 7 天）；
    /// - `Once` → 24h（旧版本会每天跑一次；新版本按一次性语义精确执行，
    ///   执行完即停用，重叠窗口极小）。
    pub fn equivalent_interval_secs(&self) -> u64 {
        match self {
            Schedule::Every { secs } => (*secs).max(1),
            Schedule::Daily { .. } => 24 * 3600,
            Schedule::Weekly { weekdays, .. } => {
                let n = weekdays
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                if n == 0 {
                    7 * 24 * 3600
                } else {
                    (7 * 24 * 3600) / n as u64
                }
            }
            Schedule::Once { .. } => 24 * 3600,
        }
    }

    /// 最近一个「≤ now」的**绝对**时刻点（epoch 秒）。
    ///
    /// - [`Schedule::Every`] 返回 `None`：它是**相对**语义，锚点由调用方用
    ///   `last_run + secs` 算（见 `TaskScheduler::plan_due`）；
    /// - `Daily` / `Weekly` / `Once` 是绝对时刻点，与上次执行无关；
    /// - 参数非法（时刻字符串解析失败）→ `None`（上层按「不触发」处理并告警）。
    pub fn most_recent_occurrence(&self, now: i64) -> Option<i64> {
        let now_dt = Local.timestamp_opt(now, 0).single()?;
        match self {
            Schedule::Every { .. } => None,
            Schedule::Daily { at } => {
                let t = parse_time(at)?;
                let today = now_dt.date_naive().and_time(t);
                if let Some(ts) = local_ts(today) {
                    if ts <= now {
                        return Some(ts);
                    }
                }
                // 今天还没到 → 退回昨天
                local_ts(next_day(now_dt.date_naive(), -1).and_time(t))
            }
            Schedule::Weekly { weekdays, at } => {
                let t = parse_time(at)?;
                if weekdays.is_empty() {
                    return None;
                }
                // 从今天往回最多找 7 天（含今天）
                for back in 0..8 {
                    let date = next_day(now_dt.date_naive(), -back);
                    let iso = date.weekday().number_from_monday() as u8;
                    if !weekdays.contains(&iso) {
                        continue;
                    }
                    if let Some(ts) = local_ts(date.and_time(t)) {
                        if ts <= now {
                            return Some(ts);
                        }
                    }
                }
                None
            }
            Schedule::Once { at } => {
                let dt = parse_datetime(at)?;
                let ts = local_ts(dt)?;
                (ts <= now).then_some(ts)
            }
        }
    }

    /// 下一个「> now」的时刻点（epoch 秒）——给「下次触发」展示用。
    ///
    /// `Every` 同样返回 `None`（相对语义，调用方用 `last_run + secs`）。
    pub fn next_occurrence_after(&self, now: i64) -> Option<i64> {
        let now_dt = Local.timestamp_opt(now, 0).single()?;
        match self {
            Schedule::Every { .. } => None,
            Schedule::Daily { at } => {
                let t = parse_time(at)?;
                for day in 0..3 {
                    let date = next_day(now_dt.date_naive(), day);
                    if let Some(ts) = local_ts(date.and_time(t)) {
                        if ts > now {
                            return Some(ts);
                        }
                    }
                }
                None
            }
            Schedule::Weekly { weekdays, at } => {
                let t = parse_time(at)?;
                if weekdays.is_empty() {
                    return None;
                }
                for ahead in 0..9 {
                    let date = next_day(now_dt.date_naive(), ahead);
                    let iso = date.weekday().number_from_monday() as u8;
                    if !weekdays.contains(&iso) {
                        continue;
                    }
                    if let Some(ts) = local_ts(date.and_time(t)) {
                        if ts > now {
                            return Some(ts);
                        }
                    }
                }
                None
            }
            Schedule::Once { at } => {
                let dt = parse_datetime(at)?;
                let ts = local_ts(dt)?;
                (ts > now).then_some(ts)
            }
        }
    }

    /// 人类可读描述（日志 / 状态接口用）。
    pub fn describe(&self) -> String {
        match self {
            Schedule::Every { secs } => format!("每 {secs} 秒"),
            Schedule::Daily { at } => format!("每天 {at}"),
            Schedule::Weekly { weekdays, at } => {
                let names: Vec<&str> = weekdays
                    .iter()
                    .filter_map(|d| match d {
                        1 => Some("周一"),
                        2 => Some("周二"),
                        3 => Some("周三"),
                        4 => Some("周四"),
                        5 => Some("周五"),
                        6 => Some("周六"),
                        7 => Some("周日"),
                        _ => None,
                    })
                    .collect();
                format!("每{} {}（本地时间）", names.join("、"), at)
            }
            Schedule::Once { at } => format!("一次性 {at}（本地时间）"),
        }
    }
}

impl MissedPolicy {
    /// 人类可读描述（状态接口 / 日志用）。
    pub fn describe(&self) -> String {
        match self {
            MissedPolicy::RunOnce => "错过就立即补跑一次".to_string(),
            MissedPolicy::Skip => "错过就跳过，等下一个时刻".to_string(),
            MissedPolicy::Within { grace_secs } => {
                format!("{grace_secs} 秒窗口内可补跑，超出跳过")
            }
        }
    }
}

/// 解析 `HH:MM`（也接受 `HH:MM:SS`）。
fn parse_time(at: &str) -> Option<NaiveTime> {
    let s = at.trim();
    NaiveTime::parse_from_str(s, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S"))
        .ok()
}

/// 解析 `YYYY-MM-DDTHH:MM`（也接受带秒 / 空格分隔）。
fn parse_datetime(at: &str) -> Option<chrono::NaiveDateTime> {
    let s = at.trim();
    for fmt in ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt);
        }
    }
    None
}

/// 本地 naive 日期时间 → epoch 秒（夏令时空档 → `None`）。
fn local_ts(dt: chrono::NaiveDateTime) -> Option<i64> {
    Local
        .from_local_datetime(&dt)
        .single()
        .map(|d| d.timestamp())
}

/// 相对日期偏移（`days` 可为负）。
fn next_day(date: NaiveDate, days: i64) -> NaiveDate {
    if days >= 0 {
        date + ChronoDuration::days(days)
    } else {
        date - ChronoDuration::days(-days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定基准：2026-11-04 15:31:00（本地）。
    fn base() -> i64 {
        Local
            .with_ymd_and_hms(2026, 11, 4, 15, 31, 0)
            .single()
            .expect("基准时刻应可构造")
            .timestamp()
    }

    fn ts_of(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("时刻应可构造")
            .timestamp()
    }

    /// Daily：15:31 时查 15:30 → 命中**今天** 15:30。
    #[test]
    fn daily_picks_today_when_time_already_passed() {
        let s = Schedule::Daily {
            at: "15:30".to_string(),
        };
        let got = s.most_recent_occurrence(base()).expect("应命中今天");
        assert_eq!(got, ts_of(2026, 11, 4, 15, 30));
        // 15:29 时查 → 命中昨天 15:30（今天还没到）
        let earlier = ts_of(2026, 11, 4, 15, 29);
        assert_eq!(
            s.most_recent_occurrence(earlier),
            Some(ts_of(2026, 11, 3, 15, 30))
        );
    }

    /// Daily 的「下一次」：15:31 时 → 明天 15:30。
    #[test]
    fn daily_next_is_tomorrow() {
        let s = Schedule::Daily {
            at: "15:30".to_string(),
        };
        assert_eq!(
            s.next_occurrence_after(base()),
            Some(ts_of(2026, 11, 5, 15, 30))
        );
    }

    /// Weekly：选中周一/周三/周五 09:00，从周三 15:31 往回找 → 周三 09:00。
    #[test]
    fn weekly_picks_latest_matching_day() {
        // 2026-11-04 是周三
        let s = Schedule::Weekly {
            weekdays: vec![1, 3, 5],
            at: "09:00".to_string(),
        };
        assert_eq!(
            s.most_recent_occurrence(base()),
            Some(ts_of(2026, 11, 4, 9, 0))
        );
        // 下一次：周五 09:00
        assert_eq!(
            s.next_occurrence_after(base()),
            Some(ts_of(2026, 11, 6, 9, 0))
        );
    }

    /// Weekly 空列表 → 永不触发（返回 None，而不是当作每天）。
    #[test]
    fn weekly_empty_is_never() {
        let s = Schedule::Weekly {
            weekdays: vec![],
            at: "09:00".to_string(),
        };
        assert_eq!(s.most_recent_occurrence(base()), None);
        assert_eq!(s.next_occurrence_after(base()), None);
    }

    /// Once：还没到 → 不触发；到了 → 触发且不再有下一次。
    #[test]
    fn once_fires_only_after_its_instant() {
        let s = Schedule::Once {
            at: "2026-11-05T15:30".to_string(),
        };
        assert_eq!(s.most_recent_occurrence(base()), None, "未来的一次性不触发");
        assert_eq!(
            s.next_occurrence_after(base()),
            Some(ts_of(2026, 11, 5, 15, 30))
        );
        let after = ts_of(2026, 11, 5, 16, 0);
        assert_eq!(
            s.most_recent_occurrence(after),
            Some(ts_of(2026, 11, 5, 15, 30))
        );
        assert_eq!(s.next_occurrence_after(after), None, "一次性没有下一次");
    }

    /// Every 是相对语义：`most_recent_occurrence` 返回 None（锚点由调度器算）。
    #[test]
    fn every_has_no_absolute_occurrence() {
        let s = Schedule::Every { secs: 1800 };
        assert_eq!(s.most_recent_occurrence(base()), None);
        assert_eq!(s.next_occurrence_after(base()), None);
        assert_eq!(s.as_interval_secs(), Some(1800));
    }

    /// 错过策略：RunOnce 恒触发；Skip 只容忍一个 tick；Within 按窗口。
    #[test]
    fn missed_policy_decisions() {
        assert_eq!(
            decide_fire(MissedPolicy::RunOnce, 8 * 3600, 60),
            FireDecision::Fire { delay: 8 * 3600 }
        );
        assert_eq!(
            decide_fire(MissedPolicy::Skip, 30, 60),
            FireDecision::Fire { delay: 30 },
            "准点后一个 tick 内仍算准点"
        );
        assert_eq!(
            decide_fire(MissedPolicy::Skip, 61, 60),
            FireDecision::Skip { delay: 61 },
            "超过一个 tick 即视为错过"
        );
        assert_eq!(
            decide_fire(MissedPolicy::Within { grace_secs: 3600 }, 3599, 60),
            FireDecision::Fire { delay: 3599 }
        );
        assert_eq!(
            decide_fire(MissedPolicy::Within { grace_secs: 3600 }, 3601, 60),
            FireDecision::Skip { delay: 3601 }
        );
    }

    /// 落盘双写用的等价间隔：旧版本读新文件时「大致正确」而不是瘫痪。
    #[test]
    fn equivalent_interval_for_dual_write() {
        assert_eq!(
            Schedule::Every { secs: 600 }.equivalent_interval_secs(),
            600
        );
        assert_eq!(
            Schedule::Daily { at: "15:30".into() }.equivalent_interval_secs(),
            24 * 3600
        );
        assert_eq!(
            Schedule::Weekly {
                weekdays: vec![1],
                at: "09:00".into()
            }
            .equivalent_interval_secs(),
            7 * 24 * 3600
        );
        assert_eq!(
            Schedule::Weekly {
                weekdays: vec![1, 3, 5],
                at: "09:00".into()
            }
            .equivalent_interval_secs(),
            (7 * 24 * 3600) / 3
        );
        assert!(
            Schedule::Once {
                at: "2026-11-05T15:30".into()
            }
            .equivalent_interval_secs()
                >= 3600,
            "一次性任务的等价间隔不能是 0（旧版本会每 tick 都跑）"
        );
    }

    /// JSON 形状：`tag = "kind"`，前端/GUI 直接按 kind 分派。
    #[test]
    fn json_roundtrip_shape() {
        let s = Schedule::Weekly {
            weekdays: vec![1, 3],
            at: "09:00".into(),
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["kind"], "weekly");
        assert_eq!(json["at"], "09:00");
        let back: Schedule = serde_json::from_value(json).unwrap();
        assert_eq!(back, s);

        let m = MissedPolicy::Within { grace_secs: 7200 };
        let json = serde_json::to_value(m).unwrap();
        assert_eq!(json["kind"], "within");
        assert_eq!(json["grace_secs"], 7200);
        let back: MissedPolicy = serde_json::from_value(json).unwrap();
        assert_eq!(back, m);

        // 旧语义可读：缺省 missed → RunOnce
        let p: MissedPolicy = serde_json::from_str(r#"{"kind":"run_once"}"#).unwrap();
        assert_eq!(p, MissedPolicy::RunOnce);
    }

    /// 非法时刻字符串不 panic，返回 None（上层按「不触发」处理）。
    #[test]
    fn invalid_time_strings_are_none() {
        for bad in ["", "25:99", "abc", "15"] {
            let s = Schedule::Daily {
                at: bad.to_string(),
            };
            assert_eq!(s.most_recent_occurrence(base()), None, "at = {bad:?}");
        }
        let s = Schedule::Once {
            at: "not-a-date".into(),
        };
        assert_eq!(s.most_recent_occurrence(base()), None);
    }
}
