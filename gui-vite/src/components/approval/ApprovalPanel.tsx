import { useState, useEffect, useCallback } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { toErrorMessage } from '@/lib/errors';
import { ErrorBanner } from '@/components/ui/ErrorBanner';
import { fetchApprovalStatus, respondApproval } from '@/lib/api-client';
import { formatDateTime } from '@/lib/utils';
import type { ApprovalDecision, ApprovalStatusSnapshot } from '@/lib/types';
import { Loader2, ShieldCheck, RefreshCw, Check, X, ShieldAlert, Pencil } from 'lucide-react';

/** 风险等级 → 中文标签。 */
const RISK_LABELS: Record<string, string> = {
  Safe: '安全',
  Low: '低',
  Medium: '中',
  High: '高',
  Critical: '危险',
};

/** 风险等级 → badge 颜色（Tailwind 明暗双主题）。 */
const RISK_BADGE_CLASSES: Record<string, string> = {
  Safe: 'bg-green-50 dark:bg-green-900/30 text-green-700 dark:text-green-300 border-green-200 dark:border-green-800',
  Low: 'bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-800',
  Medium:
    'bg-yellow-50 dark:bg-yellow-900/30 text-yellow-700 dark:text-yellow-300 border-yellow-200 dark:border-yellow-800',
  High: 'bg-orange-50 dark:bg-orange-900/30 text-orange-700 dark:text-orange-300 border-orange-200 dark:border-orange-800',
  Critical:
    'bg-red-50 dark:bg-red-900/30 text-red-700 dark:text-red-300 border-red-200 dark:border-red-800',
};

/** 审批倒计时（P1：交互模式阻塞期间用户需知道还剩多久自动处理）。 */
function CountdownText({ requestedAt, timeoutSecs }: { requestedAt: string; timeoutSecs: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const deadline = new Date(requestedAt).getTime() + timeoutSecs * 1000;
  const remainMs = Math.max(deadline - now, 0);
  const remainSec = Math.ceil(remainMs / 1000);
  const mm = Math.floor(remainSec / 60);
  const ss = remainSec % 60;
  return (
    <span
      title={`请求于 ${formatDateTime(requestedAt)}，超时 ${timeoutSecs}s 自动处理`}
      className="text-amber-600 dark:text-amber-400"
    >
      {remainSec > 0 ? `剩余 ${mm}:${String(ss).padStart(2, '0')} 自动处理` : '超时处理中'}
    </span>
  );
}

/** 决策 → 中文标签（后端返回大小写不固定，统一小写匹配）。 */
const DECISION_LABELS: Record<string, string> = {
  approve: '批准',
  deny: '拒绝',
  request_more_info: '更多信息',
};

/** 风险等级徽标。 */
function RiskBadge({ riskLevel }: { riskLevel: string }) {
  const label = RISK_LABELS[riskLevel] ?? riskLevel;
  const classes = RISK_BADGE_CLASSES[riskLevel] ?? RISK_BADGE_CLASSES.Medium;
  return (
    <span className={`inline-flex items-center px-1.5 py-0.5 text-xs rounded border ${classes}`}>
      {label}
    </span>
  );
}

/** 审批决策 → 中文标签（response 可能为 null）。 */
function decisionLabel(decision: string | null | undefined): string {
  if (!decision) return '未响应';
  const key = decision.toLowerCase();
  return DECISION_LABELS[key] ?? decision;
}

/** 布尔配置 → 中文显示。 */
function booleanLabel(value: boolean): string {
  return value ? '开启' : '关闭';
}

/**
 * 从审批请求的 action 中提取命令文本（execute_command 类请求）。
 *
 * 后端 Action 序列化为 `{ action_type: "ExecuteCommand", command: ... }`
 * （serde tag = "action_type"）；其他类型返回 null，不显示编辑交互。
 */
function extractCommand(action: unknown): string | null {
  if (!action || typeof action !== 'object') return null;
  const a = action as Record<string, unknown>;
  if (a.action_type === 'ExecuteCommand' && typeof a.command === 'string') {
    return a.command;
  }
  return null;
}

export default function ApprovalPanel() {
  const [snapshot, setSnapshot] = useState<ApprovalStatusSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 正在响应的请求_id，用于禁用按钮防重复点击
  const [respondingId, setRespondingId] = useState<string | null>(null);
  // 正在内联编辑命令的 request_id（null = 未在编辑）
  const [editingId, setEditingId] = useState<string | null>(null);
  // 内联编辑草稿
  const [editText, setEditText] = useState('');

  // 挂载后立即加载 + 每 2s 轮询（审批需及时出现）；失败显错（与任务面板
  // 的静默策略不同——审批超时会影响安全性，错误需要可见 + 可重试）
  const pollStatus = useCallback(async () => {
    try {
      const res = await fetchApprovalStatus();
      setSnapshot(res);
      setError(null);
    } catch (err: unknown) {
      setError(toErrorMessage(err, '加载审批状态失败'));
    }
  }, []);
  const { loading, refresh } = usePolling(pollStatus, 2000);

  const handleRespond = useCallback(
    async (requestId: string, decision: ApprovalDecision, editedCommand?: string) => {
      setRespondingId(requestId);
      try {
        await respondApproval(requestId, decision, undefined, editedCommand);
        await refresh();
        setEditingId(null);
      } catch (err: unknown) {
        setError(toErrorMessage(err, '响应审批失败'));
      } finally {
        setRespondingId(null);
      }
    },
    [refresh],
  );

  // 开始内联编辑：预填原命令
  const startEdit = useCallback((requestId: string, command: string) => {
    setEditingId(requestId);
    setEditText(command);
  }, []);

  const pending = snapshot?.pending_approvals ?? [];
  const confirmations = snapshot?.pending_confirmations ?? [];
  const records = snapshot?.recent_records.slice(0, 10) ?? [];
  const interactiveMode = snapshot?.config.mode === 'interactive';

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-[var(--color-border)]">
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">审批</h2>
        <button
          onClick={() => void refresh(true)}
          disabled={loading}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
        >
          <RefreshCw size={12} />
          刷新
        </button>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-6 space-y-6">
        {error && <ErrorBanner message={error} onRetry={() => void refresh(true)} />}

        {!error && loading && (
          <div className="flex items-center justify-center py-16">
            <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
          </div>
        )}

        {!error && !loading && snapshot && (
          <>
            {/* a) 待处理审批 */}
            <section>
              <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                待处理审批
              </h3>
              {pending.length === 0 ? (
                <div className="flex flex-col items-center justify-center py-10 text-[var(--color-text-tertiary)]">
                  <ShieldCheck size={36} className="mb-2 opacity-40" />
                  <p className="text-sm">暂无待处理审批</p>
                  {!interactiveMode && (
                    <p className="text-xs mt-2 max-w-md text-center">
                      当前为「{snapshot?.config.mode}
                      」模式；交互模式下危险操作会在此面板等待人工响应
                    </p>
                  )}
                </div>
              ) : (
                <div className="space-y-2">
                  {pending.map((req) => {
                    const responding = respondingId === req.request_id;
                    const command = extractCommand(req.action);
                    const editing = editingId === req.request_id;
                    return (
                      <div
                        key={req.request_id}
                        className="p-4 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
                      >
                        <div className="flex items-start justify-between gap-3">
                          <div className="min-w-0 flex-1">
                            <p className="text-sm text-[var(--color-text-primary)] break-all">
                              {req.action_description}
                            </p>
                            <div className="flex items-center gap-2 mt-1.5 text-xs text-[var(--color-text-tertiary)]">
                              <RiskBadge riskLevel={req.risk_level} />
                              <span>{formatDateTime(req.requested_at)}</span>
                              <CountdownText
                                requestedAt={req.requested_at}
                                timeoutSecs={req.timeout_secs}
                              />
                            </div>
                            {/* execute_command 类请求：展示命令文本 + 编辑后批准 */}
                            {command !== null && (
                              <pre className="mt-2 px-2.5 py-1.5 text-xs text-[var(--color-text-primary)] bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded overflow-x-auto whitespace-pre-wrap break-all font-mono">
                                {command}
                              </pre>
                            )}
                            {editing && command !== null && (
                              <div className="mt-2">
                                <textarea
                                  value={editText}
                                  onChange={(e) => setEditText(e.target.value)}
                                  rows={2}
                                  aria-label="编辑后的命令"
                                  className="w-full px-2.5 py-1.5 text-xs font-mono text-[var(--color-text-primary)] bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded resize-y focus:outline-none focus:border-[var(--color-accent)]"
                                />
                                <div className="flex items-center gap-2 mt-1.5">
                                  <button
                                    onClick={() =>
                                      void handleRespond(req.request_id, 'approve', editText)
                                    }
                                    disabled={responding || editText.trim() === ''}
                                    aria-label="提交编辑并批准"
                                    className="flex items-center gap-1 px-2.5 py-1 text-xs rounded bg-green-600 text-white hover:bg-green-700 disabled:opacity-50"
                                  >
                                    {responding ? (
                                      <Loader2 size={12} className="animate-spin" />
                                    ) : (
                                      <Check size={12} />
                                    )}
                                    提交编辑并批准
                                  </button>
                                  <button
                                    onClick={() => setEditingId(null)}
                                    disabled={responding}
                                    className="px-2.5 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
                                  >
                                    取消
                                  </button>
                                </div>
                              </div>
                            )}
                          </div>
                          <div className="flex items-center gap-2 shrink-0">
                            {command !== null && !editing && (
                              <button
                                onClick={() => startEdit(req.request_id, command)}
                                disabled={responding}
                                aria-label={`编辑后批准 ${req.action_description}`}
                                className="flex items-center gap-1 px-2.5 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
                              >
                                <Pencil size={12} />
                                编辑后批准
                              </button>
                            )}
                            <button
                              onClick={() => void handleRespond(req.request_id, 'approve')}
                              disabled={responding}
                              aria-label={`批准 ${req.action_description}`}
                              className="flex items-center gap-1 px-2.5 py-1 text-xs rounded bg-green-600 text-white hover:bg-green-700 disabled:opacity-50"
                            >
                              {responding ? (
                                <Loader2 size={12} className="animate-spin" />
                              ) : (
                                <Check size={12} />
                              )}
                              批准
                            </button>
                            <button
                              onClick={() => void handleRespond(req.request_id, 'deny')}
                              disabled={responding}
                              aria-label={`拒绝 ${req.action_description}`}
                              className="flex items-center gap-1 px-2.5 py-1 text-xs rounded bg-red-600 text-white hover:bg-red-700 disabled:opacity-50"
                            >
                              <X size={12} />
                              拒绝
                            </button>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </section>

            {/* b) 待确认操作（ask-user 降级链路的对话确认指纹） */}
            <section>
              <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                待确认操作
                <span
                  aria-label={`待确认操作数：${confirmations.length}`}
                  className="ml-1.5 inline-flex items-center px-1.5 py-0.5 text-[10px] rounded-full bg-[var(--color-bg-primary)] border border-[var(--color-border)] text-[var(--color-text-secondary)]"
                >
                  {confirmations.length}
                </span>
              </h3>
              {confirmations.length === 0 ? (
                <div className="flex items-center gap-2 px-3 py-2 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-tertiary)]">
                  <ShieldAlert size={14} className="shrink-0 opacity-40" />
                  <p className="text-sm">无待确认操作</p>
                </div>
              ) : (
                <div className="space-y-2">
                  {confirmations.map((fingerprint, idx) => (
                    <pre
                      key={`${fingerprint}-${idx}`}
                      className="px-2.5 py-1.5 text-xs text-[var(--color-text-primary)] bg-[var(--color-bg-secondary)] border border-[var(--color-border)] rounded whitespace-pre-wrap break-all font-mono"
                    >
                      {fingerprint}
                    </pre>
                  ))}
                </div>
              )}
            </section>

            {/* c) 审批配置摘要 */}
            <section>
              <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                审批配置
              </h3>
              <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="text-xs text-[var(--color-text-tertiary)]">审批模式</p>
                  <p className="text-sm font-medium text-[var(--color-text-primary)] mt-1">
                    {snapshot.config.mode}
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="text-xs text-[var(--color-text-tertiary)]">自动批准</p>
                  <p className="text-sm font-medium text-[var(--color-text-primary)] mt-1">
                    {booleanLabel(snapshot.config.enable_auto_approval)}
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="text-xs text-[var(--color-text-tertiary)]">默认超时</p>
                  <p className="text-sm font-medium text-[var(--color-text-primary)] mt-1">
                    {snapshot.config.default_timeout_secs} 秒
                  </p>
                </div>
                <div className="p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)]">
                  <p className="text-xs text-[var(--color-text-tertiary)]">已确认操作数</p>
                  <p className="text-sm font-medium text-[var(--color-text-primary)] mt-1">
                    {snapshot.confirmed_action_count}
                  </p>
                </div>
              </div>
            </section>

            {/* d) 最近审计（前 10 条） */}
            <section>
              <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider mb-2">
                最近审计
              </h3>
              {records.length === 0 ? (
                <div className="flex items-center justify-center py-8 text-[var(--color-text-tertiary)]">
                  <ShieldAlert size={28} className="mr-2 opacity-40" />
                  <p className="text-sm">暂无审计记录</p>
                </div>
              ) : (
                <div className="space-y-1">
                  {records.map((record) => (
                    <div
                      key={record.request.request_id}
                      className="flex items-center gap-3 px-3 py-2 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
                    >
                      <div className="min-w-0 flex-1">
                        <p className="text-sm text-[var(--color-text-primary)] truncate">
                          {record.request.action_description}
                        </p>
                        {record.edited_command && (
                          <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5 truncate">
                            编辑后命令：{record.edited_command}
                          </p>
                        )}
                        <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
                          {record.response ? formatDateTime(record.response.responded_at) : '—'}
                        </p>
                      </div>
                      <div className="flex items-center gap-2 shrink-0">
                        <span className="px-1.5 py-0.5 text-xs rounded bg-[var(--color-bg-primary)] border border-[var(--color-border)] text-[var(--color-text-secondary)]">
                          {decisionLabel(record.response?.decision)}
                        </span>
                        <span className="text-xs text-[var(--color-text-secondary)]">
                          {record.response?.approved_by ?? '—'}
                        </span>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </section>
          </>
        )}
      </div>
    </div>
  );
}
