//! 定时智能体任务类型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 定时智能体任务（**持久化模型**）。
///
/// 时间字段是 epoch 秒 —— 这是 `scheduled_agent_tasks.json` 的落盘格式，**刻意不改它**：
/// 换成 RFC3339 会让旧版本读到新文件时整个反序列化失败，升级就变成不可逆的
/// （回退要手工改 JSON）。对外契约见 [`ScheduledTaskResponse`]（统一 RFC3339）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledAgentTask {
    /// 任务唯一 ID。
    pub id: String,
    /// 任务显示名称。
    pub name: String,
    /// 执行间隔（秒）：距上次执行 ≥ 此值即在下个扫描周期执行（ADR-024 间隔制；
    /// 宕机补跑——服务未运行期间超期的任务在重启后自动执行一次）。
    /// 旧 JSON 无此字段 → default 0 → load 时从 cron 换算。
    #[serde(default)]
    pub interval_secs: u64,
    /// 旧 cron 表达式（ADR-024 迁移遗留；load 时换算为 interval_secs 后不再使用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// 工作目录（agent 执行命令/读写文件的根）。
    pub workspace: String,
    /// 给智能体的指令（到点后让它在这个工作区做的事）。
    pub prompt: String,
    /// 是否启用。
    pub enabled: bool,
    /// 创建时间（epoch 秒）。
    pub created_at: i64,
    /// 上次执行时间（epoch 秒；未执行过为 None）。
    pub last_run_at: Option<i64>,
    /// 下次预计执行时间（epoch 秒；展示用近似值 = last_run_at + interval）。
    pub next_run_at: Option<i64>,
    /// 最近一次执行结果摘要。
    pub last_result: Option<String>,
    /// **何时该跑**（间隔 / 每天 / 每周 / 一次性）。
    ///
    /// `None` = 旧数据（或旧客户端创建）：按 `interval_secs` 推成
    /// [`tianyan::scheduler::Schedule::Every`]，即 ADR-024 的间隔语义。
    /// 落盘与 `interval_secs` **双写**（见 [`Self::sync_legacy_interval`]），
    /// 旧版本读新文件仍能跑（可回退）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<tianyan::scheduler::Schedule>,
    /// **错过了怎么办**（立即补跑 / 跳过 / 窗口内补跑）。
    ///
    /// `None` = 旧行为（[`tianyan::scheduler::MissedPolicy::RunOnce`]：开机即补）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missed: Option<tianyan::scheduler::MissedPolicy>,
}

impl ScheduledAgentTask {
    /// 创建新任务。
    pub fn new(name: String, interval_secs: u64, workspace: String, prompt: String) -> Self {
        Self {
            id: format!("sched-task-{}", uuid::Uuid::new_v4()),
            name,
            interval_secs,
            cron: None,
            workspace,
            prompt,
            enabled: true,
            created_at: Utc::now().timestamp(),
            last_run_at: None,
            next_run_at: None,
            last_result: None,
            schedule: None,
            missed: None,
        }
    }

    /// 生效的调度规格：显式 `schedule` 优先，否则由 `interval_secs` 推成间隔制。
    ///
    /// 这是**唯一**的语义出口——判定、注册、`next_run` 计算都读它，避免
    /// 「一处看 `schedule`、另一处看 `interval_secs`」的分叉。
    pub fn effective_schedule(&self) -> tianyan::scheduler::Schedule {
        self.schedule
            .clone()
            .unwrap_or(tianyan::scheduler::Schedule::Every {
                secs: self.interval_secs.max(1),
            })
    }

    /// 生效的错过策略（缺省沿用旧行为：错过就立即补跑一次）。
    pub fn effective_missed(&self) -> tianyan::scheduler::MissedPolicy {
        self.missed.unwrap_or_default()
    }

    /// 把 `schedule` 的等价间隔同步进 `interval_secs`（**落盘双写**）。
    ///
    /// 旧版本只认 `interval_secs`：不双写的话，新版本创建的任务在旧版本里读成
    /// `interval_secs = 0`（`load` 会拿它去换算 cron → 回退每日一次），升级/回退
    /// 行为会漂。双写后旧版本「大致正确」，新版本按 `schedule` 精确执行。
    pub fn sync_legacy_interval(&mut self) {
        self.interval_secs = self.effective_schedule().equivalent_interval_secs();
    }

    /// 估算「下次触发」的 epoch 秒（展示用；真实触发以调度器判据为准）。
    ///
    /// - 间隔制：`now + 间隔`（与旧实现一致）；
    /// - 日历类：按**时刻点**算（`next_occurrence_after`）——用 `last_run + 间隔`
    ///   算日历任务会给出错误时间（这正是「收盘任务被显示成次日盘中」的来源）。
    pub fn estimated_next_run(&self, now: i64) -> Option<i64> {
        match self.effective_schedule() {
            tianyan::scheduler::Schedule::Every { secs } => Some(now + secs as i64),
            schedule => schedule.next_occurrence_after(now),
        }
    }
}

/// 定时任务 API 响应（**对外契约：时间统一 RFC3339**）。
///
/// 产品其余接口（sessions / skills / memory / goals / todos）一律 RFC3339，只有这里
/// 原本输出 epoch 秒，前端因此得为它单独维护一条解析分支。统一放在这一层做：
/// **落盘格式不动，只改对外表示** —— 既消掉了「两套时间格式」，又保住可回退。
#[derive(Debug, Clone, Serialize)]
pub struct ScheduledTaskResponse {
    /// 任务唯一 ID。
    pub id: String,
    /// 任务显示名称。
    pub name: String,
    /// 执行间隔（秒）。
    pub interval_secs: u64,
    /// 工作目录。
    pub workspace: String,
    /// 给智能体的指令。
    pub prompt: String,
    /// 是否启用。
    pub enabled: bool,
    /// 创建时间（RFC3339）。
    pub created_at: DateTime<Utc>,
    /// 上次执行时间（RFC3339；未执行过为 None）。
    pub last_run_at: Option<DateTime<Utc>>,
    /// 下次预计执行时间（RFC3339）。
    pub next_run_at: Option<DateTime<Utc>>,
    /// 最近一次执行结果摘要。
    pub last_result: Option<String>,
    /// 调度规格的可读描述（如「每天 15:30」）。
    pub schedule_desc: String,
    /// 错过策略的可读描述。
    pub missed_desc: String,
    /// **结构化**调度规格（= `effective_schedule()`，`{"kind":"daily","at":"15:30"}`）。
    ///
    /// 编辑表单必须拿到原样的结构才能回填（`schedule_desc` 是给人看的字符串，反解
    /// 不了）。缺省字段（旧任务）在这里同样是**生效值**——调用方不必再自己兜
    /// 「`schedule` 为空 → 按间隔制推」那套语义。
    pub schedule: tianyan::scheduler::Schedule,
    /// **结构化**错过策略（= `effective_missed()`）。
    pub missed: tianyan::scheduler::MissedPolicy,
}

impl From<&ScheduledAgentTask> for ScheduledTaskResponse {
    fn from(task: &ScheduledAgentTask) -> Self {
        // epoch → UTC；越界值（理论上不会出现）退化为「现在」，不 panic
        let to_dt = |secs: i64| DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_else(Utc::now);
        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            interval_secs: task.interval_secs,
            workspace: task.workspace.clone(),
            prompt: task.prompt.clone(),
            enabled: task.enabled,
            created_at: to_dt(task.created_at),
            last_run_at: task.last_run_at.map(to_dt),
            next_run_at: task.next_run_at.map(to_dt),
            last_result: task.last_result.clone(),
            schedule_desc: task.effective_schedule().describe(),
            missed_desc: task.effective_missed().describe(),
            schedule: task.effective_schedule(),
            missed: task.effective_missed(),
        }
    }
}

/// 创建定时任务请求。
#[derive(Debug, Clone, Deserialize)]
pub struct CreateScheduledTaskRequest {
    /// 任务显示名称。
    pub name: String,
    /// 执行间隔（秒）——仅当没给 `schedule` 时使用（旧客户端兼容）。
    ///
    /// 如 1800 = 每 30 分钟、86400 = 每天（距上次执行 ≥24h 即触发）。
    #[serde(default)]
    pub interval_secs: u64,
    /// 调度规格（推荐）：`{"kind":"daily","at":"15:30"}` /
    /// `{"kind":"weekly","weekdays":[1,3,5],"at":"09:00"}` /
    /// `{"kind":"once","at":"2026-11-05T15:30"}` / `{"kind":"every","secs":1800}`。
    #[serde(default)]
    pub schedule: Option<tianyan::scheduler::Schedule>,
    /// 错过策略：`{"kind":"skip"}` / `{"kind":"run_once"}` /
    /// `{"kind":"within","grace_secs":28800}`。缺省 = 旧的「立即补跑一次」。
    #[serde(default)]
    pub missed: Option<tianyan::scheduler::MissedPolicy>,
    /// 工作目录（agent 执行命令/读写文件的根）。
    pub workspace: String,
    /// 给智能体的指令。
    pub prompt: String,
}

/// 更新定时任务请求（**部分字段**语义：`None` = 保持原值）。
///
/// 为什么不复用 [`CreateScheduledTaskRequest`]：创建要求给全（name / workspace /
/// prompt 必填），而编辑场景里用户常常只改一处（改个提示词、换个时刻、停用一下）。
/// 用创建请求当更新请求会逼调用方把没打算改的字段原样抄回来——抄漏一个就把工作区
/// 或指令**清空**了。
///
/// 字段语义：
/// - `name` / `prompt`：给了就必须非空（去空白后），否则 `invalid_input`；
/// - `workspace`：允许空串（= 默认目录）；
/// - `schedule`：给了就整体替换；`interval_secs` 只有在**没给** `schedule` 时才生效
///   （等价于 `Schedule::Every { secs }`）。想把日历任务降回间隔制，传
///   `{"kind":"every","secs":N}`——JSON `null` 与缺省一样表示「不改」；
/// - `missed`：给了就替换（`Skip` / `Within` / `RunOnce`）；
/// - `enabled`：`false` 注销调度器（任务保留在列表里，可再启用）。
///
/// 落盘仍是 [`ScheduledAgentTask`]：`schedule` → `interval_secs` 双写由 manager
/// 统一做（可回退，见 [`ScheduledAgentTask::sync_legacy_interval`]）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateScheduledTaskRequest {
    /// 任务显示名称（`None` = 不改）。
    #[serde(default)]
    pub name: Option<String>,
    /// 执行间隔（秒；仅在没给 `schedule` 时生效）。
    #[serde(default)]
    pub interval_secs: Option<u64>,
    /// 调度规格（整体替换）。
    #[serde(default)]
    pub schedule: Option<tianyan::scheduler::Schedule>,
    /// 错过策略（整体替换）。
    #[serde(default)]
    pub missed: Option<tianyan::scheduler::MissedPolicy>,
    /// 工作目录（`Some("")` = 默认目录）。
    #[serde(default)]
    pub workspace: Option<String>,
    /// 给智能体的指令。
    #[serde(default)]
    pub prompt: Option<String>,
    /// 是否启用（`false` = 注销调度器）。
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ScheduledAgentTask {
        ScheduledAgentTask {
            id: "t1".to_string(),
            name: "n".to_string(),
            interval_secs: 60,
            cron: None,
            workspace: String::new(),
            prompt: "p".to_string(),
            enabled: true,
            created_at: 1_700_000_000,
            last_run_at: Some(1_700_000_100),
            next_run_at: None,
            last_result: None,
            schedule: None,
            missed: None,
        }
    }

    /// 新字段的**语义出口**：`schedule` 优先，缺省回落到旧的间隔制。
    #[test]
    fn effective_schedule_defaults_to_legacy_interval() {
        let mut task = sample();
        task.schedule = None;
        task.interval_secs = 1800;
        assert_eq!(
            task.effective_schedule(),
            tianyan::scheduler::Schedule::Every { secs: 1800 }
        );
        assert_eq!(
            task.effective_missed(),
            tianyan::scheduler::MissedPolicy::RunOnce,
            "旧数据缺省沿用「错过就补跑一次」"
        );

        task.schedule = Some(tianyan::scheduler::Schedule::Daily { at: "15:30".into() });
        task.missed = Some(tianyan::scheduler::MissedPolicy::Skip);
        assert_eq!(
            task.effective_schedule(),
            tianyan::scheduler::Schedule::Daily { at: "15:30".into() }
        );
        assert_eq!(
            task.effective_missed(),
            tianyan::scheduler::MissedPolicy::Skip
        );
    }

    /// 落盘**双写**：日历类任务要把等价间隔同步进 `interval_secs`，旧版本才能读。
    #[test]
    fn calendar_schedule_dual_writes_interval_secs() {
        let mut task = sample();
        task.schedule = Some(tianyan::scheduler::Schedule::Daily { at: "15:30".into() });
        task.sync_legacy_interval();
        assert_eq!(task.interval_secs, 24 * 3600, "日历类的等价间隔 = 一天");

        let disk = serde_json::to_string(&task).expect("序列化");
        assert!(
            disk.contains("\"interval_secs\":86400"),
            "必须双写等价间隔（可回退）：{disk}"
        );
        assert!(
            disk.contains("\"kind\":\"daily\""),
            "新字段也要落盘：{disk}"
        );
    }

    /// `next_run` 必须按调度规格算：日历类不是「现在 + 间隔」。
    #[test]
    fn estimated_next_run_uses_calendar_semantics() {
        use chrono::TimeZone as _;
        let at = |h, m| {
            chrono::Local
                .with_ymd_and_hms(2026, 11, 4, h, m, 0)
                .single()
                .expect("时刻应可构造")
                .timestamp()
        };
        let mut task = sample();
        task.schedule = Some(tianyan::scheduler::Schedule::Daily { at: "15:30".into() });
        // 10:00 时：下一次是今天 15:30，而不是「now + 24h」
        assert_eq!(task.estimated_next_run(at(10, 0)), Some(at(15, 30)));

        // 旧语义（间隔制）保持 now + 间隔
        task.schedule = None;
        task.interval_secs = 600;
        assert_eq!(task.estimated_next_run(at(10, 0)), Some(at(10, 0) + 600));
    }

    /// 落盘保持 epoch 秒（旧版本能读 → 可回退），对外输出 RFC3339（统一口径）。
    #[test]
    fn persistent_keeps_epoch_but_response_is_rfc3339() {
        let task = sample();
        let disk = serde_json::to_string(&task).expect("序列化");
        assert!(
            disk.contains("\"created_at\":1700000000"),
            "落盘必须保持 epoch 秒：{disk}"
        );

        let resp = ScheduledTaskResponse::from(&task);
        let api = serde_json::to_string(&resp).expect("序列化");
        assert!(
            api.contains("\"created_at\":\"20"),
            "API 应输出 RFC3339 字符串：{api}"
        );
        assert_eq!(resp.created_at.timestamp(), 1_700_000_000);
        assert_eq!(
            resp.last_run_at.map(|d| d.timestamp()),
            Some(1_700_000_100),
            "last_run_at 也要转换"
        );
        assert!(resp.next_run_at.is_none());
    }

    /// 更新请求是**部分字段**语义：只传一处也能解析（其余 `None` = 不改）。
    ///
    /// 判别力：若照搬创建请求（name/workspace/prompt 必填），下面的 JSON 会解析失败
    /// ——编辑一个提示词就得先读全量再抄回来。
    #[test]
    fn update_request_accepts_partial_fields() {
        let only_prompt: UpdateScheduledTaskRequest =
            serde_json::from_str(r#"{"prompt":"换成新的指令"}"#).expect("只改指令应可解析");
        assert_eq!(only_prompt.prompt.as_deref(), Some("换成新的指令"));
        assert!(only_prompt.name.is_none());
        assert!(only_prompt.workspace.is_none());
        assert!(only_prompt.schedule.is_none());
        assert!(only_prompt.missed.is_none());
        assert!(only_prompt.enabled.is_none());

        // 调度规格 + 启停（前端「编辑」与「启用/停用」按钮共用一个端点）
        let sched: UpdateScheduledTaskRequest = serde_json::from_str(
            r#"{"schedule":{"kind":"daily","at":"16:05"},"missed":{"kind":"skip"},"enabled":false}"#,
        )
        .expect("调度规格应可解析");
        assert_eq!(
            sched.schedule,
            Some(tianyan::scheduler::Schedule::Daily { at: "16:05".into() })
        );
        assert_eq!(sched.missed, Some(tianyan::scheduler::MissedPolicy::Skip));
        assert_eq!(sched.enabled, Some(false));

        // 旧客户端只给 interval_secs：解析成间隔制字段（由 manager 落到 schedule）
        let legacy: UpdateScheduledTaskRequest =
            serde_json::from_str(r#"{"interval_secs":3600}"#).expect("旧字段应可解析");
        assert_eq!(legacy.interval_secs, Some(3600));
        assert!(legacy.schedule.is_none(), "两者互斥：schedule 优先");
    }

    /// 响应必须同时给出**结构化**的生效调度与错过策略（编辑表单要回填原样结构，
    /// 光有人类可读的 `schedule_desc` 反解不回来）。
    #[test]
    fn response_exposes_structured_schedule_and_missed() {
        // 旧任务（无 schedule/missed 字段）→ 生效值应是间隔制 + 「错过补跑」
        let legacy = sample();
        let resp = ScheduledTaskResponse::from(&legacy);
        assert_eq!(
            resp.schedule,
            tianyan::scheduler::Schedule::Every { secs: 60 }
        );
        assert_eq!(resp.missed, tianyan::scheduler::MissedPolicy::RunOnce);

        // 显式设置 → 原样输出（前端按 kind 分派回填）
        let mut task = sample();
        task.schedule = Some(tianyan::scheduler::Schedule::Weekly {
            weekdays: vec![1, 3],
            at: "09:00".into(),
        });
        task.missed = Some(tianyan::scheduler::MissedPolicy::Within {
            grace_secs: 8 * 3600,
        });
        let json = serde_json::to_value(ScheduledTaskResponse::from(&task)).expect("序列化");
        assert_eq!(json["schedule"]["kind"], "weekly");
        assert_eq!(json["schedule"]["weekdays"], serde_json::json!([1, 3]));
        assert_eq!(json["missed"]["kind"], "within");
        assert_eq!(json["missed"]["grace_secs"], 28_800);
    }
}
