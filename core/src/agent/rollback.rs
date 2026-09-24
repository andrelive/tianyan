//! 会话回退 / 重做（ADR-040）：排他写事务内的完整编排。
//!
//! # 职责
//! 本模块拥有「回退事务」的全过程：终止活动轮与锚点后任务（锁外可等待）、保存
//! 可逆数据、恢复工作区文件、截断时序链（锁内）。server 侧 handler 只做解析与
//! 响应映射（薄壳）——此前截断由 server 在锁外编排（`get_session` → `save_redo`
//! → `truncate` → `persist_messages`），「读链 → 改 → 写链」三段各自无锁，窗口内
//! 新轮（用户轮 / 唤醒轮）落库的消息被整表重写覆盖（库为权威 → 不可自愈）。
//!
//! # 顺序（正确性所系）
//! 1. 锁外：定位锚点 → 取消锚点后任务（**不等**终态，缩短后续持锁时间）；
//! 2. 锁内：再取消一次（覆盖轮退出后新起的任务）+ 等命令任务终态（取消通知入库，
//!    否则通知会追加到截断后的新链尾污染）；
//! 3. 保存重做数据（回退前的工作区树 + 被截断消息）——必须在恢复文件**之前**；
//! 4. **先恢复工作区文件**（可能失败的一步在前：失败即整体中止，链不动）；
//! 5. **后截断时序链**（经工作集 `rewrite`，库与缓存同步推进）。
//!
//! 「文件先、链后」把半成品挡在结构层：不可能出现「消息回退了、文件没回退」。
//! 工作区未绑定（无快照根）或该锚点无快照树时，文件回退不可用但**不阻断**消息
//! 回退——结果经 [`RollbackOutcome::workdir`] 明确回报（前端须明示）。

use std::time::{Duration, Instant};

use crate::agent::agent_core::Agent;
use crate::common::error::{Result, TianyanError};
use crate::common::types::StructuredMessage;

/// 等待被 kill 的命令任务进入终态的时限（watcher 收尾异步：杀进程后
/// `child.wait()` 返回 → 取消通知入库）。
const CANCEL_TERMINAL_WAIT: Duration = Duration::from_secs(10);
/// 终态轮询间隔。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 回退结果（ADR-040 §2）：前端据此给出准确反馈——尤其
/// [`Self::workdir`] = `None`（文件未回退）必须明示，否则用户以为文件也回退了。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RollbackOutcome {
    /// 被截断（丢弃）的消息数。
    pub truncated: usize,
    /// 恢复 / 删除的工作区文件数。
    pub restored_files: usize,
    /// 会话生效的工作目录；`None` = 未绑定工作区（**文件未回退**）。
    pub workdir: Option<String>,
    /// 可撤销回退（重做数据已保存）。
    pub redo_available: bool,
    /// 被取消的时序锚点后任务数（委托 + 命令）。
    pub cancelled_tasks: usize,
}

/// 重做结果：被回退的消息与工作区文件已恢复。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RedoOutcome {
    /// 恢复的消息数。
    pub restored_messages: usize,
    /// 恢复的工作区文件数。
    pub restored_files: usize,
}

impl Agent {
    /// 回退：把会话恢复到 `message_id` **之前**（消息 + 工作区文件），全或无。
    ///
    /// 锚点是稳定键（消息 ID）：前端展示列表经合并 / 过滤（tool / system 消息）后
    /// 索引与服务端消息列表错位，数字位置不可靠。
    ///
    /// # Errors
    /// * 会话不存在 / 锚点消息不存在 → `not_found`（调用方映射 404，不执行截断）；
    /// * 快照对象损坏（树存在但内容寻址对象缺失）→ 恢复中止、**链不截断**；
    /// * 库重写失败 → 透传（缓存不动）。
    pub(crate) async fn rollback_to(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<RollbackOutcome> {
        // ── 阶段 A（锁外，可等待）：锚点定位 + 取消锚点后任务 ──
        // 这里只做"粗定位"（供 `anchor_seq` 比较）；锁内会以库的最新链重新定位。
        let anchor_seq = {
            let session = self
                .session_manager
                .get_session(session_id)
                .await?
                .ok_or_else(|| {
                    TianyanError::not_found(format!("会话存储错误：会话未找到：{session_id}"))
                })?;
            session
                .messages
                .iter()
                .position(|m| m.id == message_id)
                .ok_or_else(|| TianyanError::not_found(format!("消息不存在：{message_id}")))?
                as i64
        };
        // 先杀不等待：缩短阶段 B 的持锁时长（命令进程的终止与通知入库在锁内收尾）
        let mut cancelled_tasks = self
            .cancel_tasks_after_anchor(session_id, anchor_seq, false)
            .await?;

        // ── 阶段 B（锁内事务）：保存可逆数据 → 恢复工作区 → 截断链 ──
        self.working_sets
            .with_session_exclusive(session_id, |ws| async move {
                // 锁内读基准：库的最新**完整链**（段化只物化「最近压缩点 → 现在」，
                // 而回退需全链）＋ 重新定位锚点（库为权威，不信任锁外读到的索引）
                let chain = ws.full_chain().await?;
                let index = chain
                    .iter()
                    .position(|m| m.id == message_id)
                    .ok_or_else(|| TianyanError::not_found(format!("消息不存在：{message_id}")))?;
                let truncated: Vec<StructuredMessage> = chain[index..].to_vec();
                // 锁内再取消一次并等终态：覆盖「等待轮退出期间」新起的任务（幂等），
                // 并保证取消通知先入库（截断时随锚点后消息一并丢弃）
                cancelled_tasks += self
                    .cancel_tasks_after_anchor(session_id, index as i64, true)
                    .await?;

                let workdir = self.resolve_working_directory(session_id).await;
                let mut restored_files = 0usize;
                let mut redo_available = false;

                if let Some(sm) = &self.snapshot_manager {
                    if let Some(wd) = workdir.clone() {
                        let sm = sm.with_workdir(wd);
                        // ③ 保存可逆数据（重做基准 = 回退前的工作区树 + 被截断消息）。
                        //    失败不阻断回退——代价只是该次回退不可撤销（可见 warn）。
                        match sm.save_redo(session_id, message_id, &truncated).await {
                            Ok(()) => redo_available = true,
                            Err(e) => tracing::warn!(
                                error = %e,
                                session = %session_id,
                                "保存重做状态失败（该次回退不可撤销）"
                            ),
                        }
                        // ④ **先恢复文件**：失败即整体中止（链保持不动）。
                        //    恢复本身「先校验后动手」（对象缺失即中止，工作区未改动）。
                        match sm.restore(session_id, message_id).await {
                            Ok(n) => restored_files = n,
                            // 该锚点无快照树（未捕获 / 已被 GC）→ 无文件可回退，
                            // 与「未绑定工作区」同类：仅回退消息，不算失败。
                            Err(e) if e.is_not_found() => tracing::warn!(
                                error = %e,
                                session = %session_id,
                                anchor = %message_id,
                                "该锚点无工作区快照，仅回退消息"
                            ),
                            Err(e) => return Err(e),
                        }
                    }
                }

                // ⑤ **后截断链**（经工作集：库 + 物化段 + seq 同步推进）
                ws.rewrite(&chain[..index]).await?;

                Ok(RollbackOutcome {
                    truncated: truncated.len(),
                    restored_files,
                    workdir: workdir.map(|p| p.display().to_string()),
                    redo_available,
                    cancelled_tasks,
                })
            })
            .await
    }

    /// 重做：恢复被回退的消息与工作区文件（与回退同一排他事务入口）。
    ///
    /// 重做数据按被回退消息的 ID 组织（一次性语义：读取即消费）。
    ///
    /// # Errors
    /// * 会话不存在 → `not_found`（404）；
    /// * 未装配快照管理器 / 未绑定工作区 / 无重做状态 → `invalid_input`（400）。
    pub(crate) async fn redo_to(&self, session_id: &str, message_id: &str) -> Result<RedoOutcome> {
        if self
            .session_manager
            .get_session(session_id)
            .await?
            .is_none()
        {
            return Err(TianyanError::not_found(format!(
                "会话存储错误：会话未找到：{session_id}"
            )));
        }
        self.working_sets
            .with_session_exclusive(session_id, |ws| async move {
                let sm = self
                    .snapshot_manager
                    .clone()
                    .ok_or_else(|| TianyanError::invalid_input("未启用工作区快照，无法重做"))?;
                let workdir = self
                    .resolve_working_directory(session_id)
                    .await
                    .ok_or_else(|| TianyanError::invalid_input("未启用工作区快照，无法重做"))?;
                let Some((messages, restored_files)) = sm
                    .with_workdir(workdir)
                    .load_redo(session_id, message_id)
                    .await?
                else {
                    return Err(TianyanError::invalid_input("没有可重做的状态"));
                };

                // 锁内读基准 → 追加到当前链尾（不覆盖锁外已落库的写入）
                let mut chain = ws.full_chain().await?;
                let restored_messages = messages.len();
                chain.extend(messages);
                ws.rewrite(&chain).await?;

                Ok(RedoOutcome {
                    restored_messages,
                    restored_files,
                })
            })
            .await
    }

    /// 取消**时序锚点之后**的任务：委托任务 `cancel`、命令任务 `kill`（杀进程树），
    /// 返回本次命中数。
    ///
    /// `wait` = `true` 时等待被 kill 的命令任务进入终态——必须在截断链**之前**
    /// 完成：watcher 收尾是异步的（杀进程 → `child.wait()` 返回 → 取消通知入库），
    /// 迟到通知会追加到截断后的新链尾（污染）。
    async fn cancel_tasks_after_anchor(
        &self,
        session_id: &str,
        anchor_seq: i64,
        wait: bool,
    ) -> Result<usize> {
        let mut cancelled = 0usize;

        // 委托任务：anchor_seq > 回退点 → 取消（终态任务幂等跳过）
        for t in self
            .background_tasks
            .snapshot()
            .await
            .iter()
            .filter(|t| t.parent_session_id == session_id && t.anchor_seq > anchor_seq)
        {
            if !t.status.is_terminal() {
                self.background_tasks.cancel(&t.id).await?;
                cancelled += 1;
            }
        }

        // 命令任务：anchor_seq > 回退点 → kill
        let cmd_tasks = self.command_tasks.list().await;
        let mut killed: Vec<String> = Vec::new();
        for t in cmd_tasks
            .iter()
            .filter(|t| t.parent_session_id == session_id && t.anchor_seq > anchor_seq)
        {
            if t.status == crate::executor::CommandTaskStatus::Running
                && self.command_tasks.kill(&t.id).await.is_ok()
            {
                killed.push(t.id.clone());
                cancelled += 1;
            }
        }

        if wait && !killed.is_empty() {
            let deadline = Instant::now() + CANCEL_TERMINAL_WAIT;
            loop {
                let tasks = self.command_tasks.list().await;
                let all_terminal = killed.iter().all(|id| {
                    tasks
                        .iter()
                        .find(|t| &t.id == id)
                        .map(|t| t.status != crate::executor::CommandTaskStatus::Running)
                        .unwrap_or(true)
                });
                if all_terminal || Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
            }
        }

        Ok(cancelled)
    }
}
