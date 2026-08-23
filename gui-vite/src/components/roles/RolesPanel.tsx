import { useState } from 'react';
import { useResource } from '@/hooks/use-resource';
import { toErrorMessage } from '@/lib/errors';
import ConfirmDialog from '@/components/ui/ConfirmDialog';
import { AlertCircle, Bot, ChevronRight, Loader2, RotateCcw, Trash2, Users } from 'lucide-react';
import { apiDelete, apiPost, getRoleDetail, getRoles, getRolesStats } from '@/lib/api-client';
import { formatTimestamp } from '@/lib/utils';

const TYPE_LABEL: Record<string, string> = {
  delegation: '委托',
  web_research: '网络调研',
  search: '搜索',
  code_edit: '代码编辑',
  verify: '验证',
  command: '命令执行',
  general: '通用',
};

const SOURCE_LABEL: Record<string, string> = {
  builtin: '内置',
  user: '用户配置',
  learned: '学习演化',
};

/** 操作失败消息（前缀 + 错误消息；非 Error 时仅前缀文案）。 */
function actionErrorMessage(err: unknown): string {
  const msg = toErrorMessage(err, '');
  return msg ? `操作失败：${msg}` : '操作失败';
}

const SOURCE_STYLE: Record<string, string> = {
  builtin:
    'bg-blue-50 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300 border-blue-200 dark:border-blue-800',
  user: 'bg-emerald-50 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300 border-emerald-200 dark:border-emerald-800',
  learned:
    'bg-purple-50 text-purple-700 dark:bg-purple-900/40 dark:text-purple-300 border-purple-200 dark:border-purple-800',
};

/**
 * 子智能体角色面板（ADR-016）：展示统一角色注册表（内置种子 / 用户配置 /
 * 学习演化三源平级），含 durable 角色会话信息；支持回退内置种子与退役删除。
 */
export default function RolesPanel() {
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [actionMessage, setActionMessage] = useState<string | null>(null);

  // 角色列表 + 使用统计（useResource：加载/错误/reload 收敛）
  const {
    data: rolesData,
    loading,
    error,
    reload: reloadRoles,
  } = useResource(() => getRoles(), [], { errorFallback: '加载角色失败' });
  const roles = rolesData?.roles ?? [];
  const { data: stats, reload: reloadStats } = useResource(() => getRolesStats(), [], {
    errorFallback: '加载角色统计失败',
  });

  const selectedRole = roles.find((r) => r.name === selectedName);

  // 详情加载（useResource + enabled：选中即取、切换重取、未选中不取）
  const {
    data: detail,
    loading: loadingDetail,
    error: detailError,
    reload: reloadDetail,
  } = useResource(() => getRoleDetail(selectedName ?? ''), [selectedName], {
    enabled: !!selectedName,
    errorFallback: '加载角色详情失败',
  });

  /** 待确认操作（ConfirmDialog 状态机；替代 window.confirm——jsdom 可测） */
  const [confirm, setConfirm] = useState<{
    title: string;
    message: string;
    action: 'reset' | 'delete';
  } | null>(null);

  // 回退内置种子（先确认）
  const requestReset = () => {
    if (!selectedName) return;
    setConfirm({
      title: '回退内置种子',
      message: `将 ${selectedName} 回退到内置种子定义（学习演化结果会被覆盖）？`,
      action: 'reset',
    });
  };
  const executeReset = async () => {
    if (!selectedName) return;
    setConfirm(null);
    setActionPending(true);
    setActionMessage(null);
    try {
      await apiPost(`/roles/${encodeURIComponent(selectedName)}/reset`, {});
      setActionMessage('已回退内置种子（下次会话边界生效）');
      void reloadRoles();
      void reloadStats();
      reloadDetail();
    } catch (err) {
      setActionMessage(actionErrorMessage(err));
    } finally {
      setActionPending(false);
    }
  };

  // 退役删除（先确认）
  const requestDelete = () => {
    if (!selectedName) return;
    setConfirm({
      title: '退役删除角色',
      message: `退役并删除角色 ${selectedName}（定义与会话一并删除）？`,
      action: 'delete',
    });
  };
  const executeDelete = async () => {
    if (!selectedName) return;
    setConfirm(null);
    setActionPending(true);
    setActionMessage(null);
    try {
      await apiDelete(`/roles/${encodeURIComponent(selectedName)}`);
      setActionMessage('角色已退役删除');
      setSelectedName(null);
      void reloadRoles();
      void reloadStats();
    } catch (err) {
      setActionMessage(actionErrorMessage(err));
    } finally {
      setActionPending(false);
    }
  };

  return (
    <div className="flex flex-col h-full">
      {/* 统计概览区（ADR-016：委托统计面板） */}
      <div className="px-4 py-3 border-b border-[var(--color-border)] flex items-center gap-4 flex-wrap shrink-0">
        {stats && stats.total_calls > 0 ? (
          <>
            <div className="flex items-center gap-2 text-sm">
              <span className="text-[var(--color-text-secondary)]">累计委托</span>
              <span className="font-semibold text-[var(--color-text-primary)]">
                {stats.total_calls} 次
              </span>
            </div>
            <div className="flex items-center gap-2 text-sm">
              <span className="text-[var(--color-text-secondary)]">成功率</span>
              <span
                className={`font-semibold ${stats.success_rate < 0.5 ? 'text-red-600 dark:text-red-400' : 'text-emerald-600 dark:text-emerald-400'}`}
              >
                {Math.round(stats.success_rate * 100)}%
              </span>
            </div>
            <div className="flex items-center gap-1.5 flex-wrap">
              <span className="text-xs text-[var(--color-text-tertiary)]">任务类型：</span>
              {stats.by_task_type.map(([type, count]) => (
                <span
                  key={type}
                  className="inline-block px-1.5 py-0.5 text-[10px] rounded bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-secondary)]"
                  title={type}
                >
                  {TYPE_LABEL[type] ?? type} {count}
                </span>
              ))}
            </div>
            <div className="flex items-center gap-1.5 flex-wrap">
              <span className="text-xs text-[var(--color-text-tertiary)]">按角色：</span>
              {stats.by_role.map((r) => (
                <span
                  key={r.name}
                  className="inline-block px-1.5 py-0.5 text-[10px] rounded bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-secondary)]"
                  title={`${r.success}/${r.calls} 次成功`}
                >
                  {r.name} {r.calls} 次
                </span>
              ))}
            </div>
          </>
        ) : (
          <span className="text-xs text-[var(--color-text-tertiary)]">
            暂无委托统计——主智能体委托子智能体后，这里会展示任务类型分布、各角色调用次数与成功率
          </span>
        )}
      </div>

      <div className="flex flex-1 min-h-0">
        {/* 左列：角色列表 */}
        <div className="w-[40%] min-w-[260px] max-w-[360px] flex flex-col border-r border-[var(--color-border)]">
          <div className="px-4 py-4 border-b border-[var(--color-border)]">
            <h2 className="text-lg font-semibold text-[var(--color-text-primary)] flex items-center gap-2">
              <Users size={20} />
              子智能体
            </h2>
            <p className="text-xs text-[var(--color-text-tertiary)] mt-1">
              角色化分工（内置 / 配置 / 学习三源平级），自演化更新
            </p>
          </div>

          <div className="flex-1 overflow-y-auto p-3">
            {loading ? (
              <div
                className="flex items-center justify-center py-16"
                aria-live="polite"
                aria-label="正在加载角色"
              >
                <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
              </div>
            ) : error ? (
              <div
                role="alert"
                className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
              >
                <AlertCircle size={16} />
                <span>{error}</span>
              </div>
            ) : roles.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
                <Bot size={40} className="mb-3 opacity-40" />
                <p className="text-sm">暂无角色</p>
                <p className="text-xs mt-1 opacity-70">
                  系统从重复成功的执行模式中自动分化子智能体
                </p>
              </div>
            ) : (
              <div className="space-y-1">
                {roles.map((role) => (
                  <button
                    key={role.name}
                    onClick={() => setSelectedName(role.name)}
                    className={`w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors ${
                      selectedName === role.name
                        ? 'bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
                        : 'text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
                    }`}
                  >
                    <div className="flex items-center justify-between">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <p className="font-medium truncate">{role.name}</p>
                          <span
                            className={`shrink-0 inline-block px-1.5 py-0.5 text-[10px] rounded-full border ${SOURCE_STYLE[role.source] ?? ''}`}
                          >
                            {SOURCE_LABEL[role.source] ?? role.source}
                          </span>
                          {role.status === 'experimental' && (
                            <span className="shrink-0 inline-block px-1.5 py-0.5 text-[10px] rounded-full bg-amber-50 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300 border border-amber-200 dark:border-amber-800">
                              试验性
                            </span>
                          )}
                        </div>
                        <p className="text-xs text-[var(--color-text-tertiary)] truncate mt-0.5">
                          {role.purpose}
                        </p>
                        <p className="text-xs text-[var(--color-text-tertiary)]/80 mt-1 flex items-center gap-2 flex-wrap">
                          <span>v{role.version}</span>
                          <span>
                            {role.tool_count == null ? '工具不限' : `${role.tool_count} 工具`}
                          </span>
                          {role.usage && role.usage.calls > 0 && (
                            <span
                              className={
                                role.usage.success_rate < 0.5
                                  ? 'text-red-600 dark:text-red-400'
                                  : ''
                              }
                              title={`${role.usage.success}/${role.usage.calls} 次成功`}
                            >
                              成功率 {Math.round(role.usage.success_rate * 100)}%
                            </span>
                          )}
                        </p>
                      </div>
                      {selectedName === role.name && (
                        <ChevronRight size={14} className="shrink-0 ml-2 text-blue-500" />
                      )}
                    </div>
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* 右列：角色详情 */}
        <div className="flex-1 flex flex-col overflow-hidden">
          {!selectedRole ? (
            <div className="flex-1 flex flex-col items-center justify-center text-[var(--color-text-tertiary)]">
              <Bot size={48} className="mb-4 opacity-30" />
              <p className="text-sm">选择一个子智能体查看详情</p>
            </div>
          ) : (
            <>
              {/* Header */}
              <div className="px-6 py-4 border-b border-[var(--color-border)]">
                <div className="flex items-center gap-2 flex-wrap">
                  <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">
                    {selectedRole.name}
                  </h2>
                  <span
                    className={`inline-block px-2 py-0.5 text-xs rounded-full border ${SOURCE_STYLE[selectedRole.source] ?? ''}`}
                  >
                    {SOURCE_LABEL[selectedRole.source] ?? selectedRole.source}
                  </span>
                  {selectedRole.status === 'experimental' && (
                    <span className="inline-block px-2 py-0.5 text-xs rounded-full bg-amber-50 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300 border border-amber-200 dark:border-amber-800">
                      试验性（只展示不可调用）
                    </span>
                  )}
                  {selectedRole.lineage && (
                    <span className="text-xs text-[var(--color-text-tertiary)]">
                      进化自 {selectedRole.lineage}
                    </span>
                  )}
                </div>
                <p className="text-sm text-[var(--color-text-secondary)] mt-1">
                  {selectedRole.purpose}
                </p>
                <div className="flex items-center gap-3 mt-2 flex-wrap text-xs text-[var(--color-text-tertiary)]">
                  <span>版本 v{selectedRole.version}</span>
                  <span>
                    {selectedRole.tool_count == null
                      ? '工具白名单：不限制'
                      : `工具白名单：${selectedRole.tool_count} 个`}
                  </span>
                  {selectedRole.model && <span>模型：{selectedRole.model}</span>}
                  {selectedRole.max_turns != null && (
                    <span>最大轮数：{selectedRole.max_turns}</span>
                  )}
                  {selectedRole.usage && selectedRole.usage.calls > 0 && (
                    <span
                      className={
                        selectedRole.usage.success_rate < 0.5
                          ? 'text-red-600 dark:text-red-400'
                          : ''
                      }
                      title={`${selectedRole.usage.success}/${selectedRole.usage.calls} 次成功 / ${selectedRole.usage.failed} 次失败`}
                    >
                      调用 {selectedRole.usage.calls} 次，成功率{' '}
                      {Math.round(selectedRole.usage.success_rate * 100)}%
                    </span>
                  )}
                </div>
              </div>

              {/* 操作栏 */}
              <div className="px-6 py-2 border-b border-[var(--color-border)] flex items-center gap-2">
                <button
                  onClick={requestReset}
                  disabled={
                    actionPending ||
                    (selectedRole.source === 'builtin' && selectedRole.version === 1)
                  }
                  className="inline-flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-lg border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                  title="回退到内置种子定义"
                >
                  <RotateCcw size={13} />
                  回退内置种子
                </button>
                <button
                  onClick={requestDelete}
                  disabled={actionPending}
                  className="inline-flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-lg border border-red-200 dark:border-red-800 text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-950 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                  title="删除角色定义与会话（退役）"
                >
                  <Trash2 size={13} />
                  退役删除
                </button>
                {actionMessage && (
                  <span className="text-xs text-[var(--color-text-tertiary)]">{actionMessage}</span>
                )}
              </div>

              {/* Content */}
              <div className="flex-1 overflow-y-auto p-6 space-y-5">
                {loadingDetail ? (
                  <div
                    className="flex items-center justify-center py-16"
                    aria-live="polite"
                    aria-label="正在加载角色详情"
                  >
                    <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
                  </div>
                ) : detailError ? (
                  <div
                    role="alert"
                    className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
                  >
                    <AlertCircle size={16} />
                    <span>{detailError}</span>
                  </div>
                ) : (
                  <>
                    <section>
                      <h3 className="text-sm font-semibold text-[var(--color-text-primary)] mb-2">
                        系统提示
                      </h3>
                      <pre className="p-3 rounded-lg bg-[var(--color-bg-tertiary)] text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap break-words max-h-80 overflow-y-auto">
                        {detail?.system_prompt || '（无系统提示）'}
                      </pre>
                    </section>
                    <section>
                      <h3 className="text-sm font-semibold text-[var(--color-text-primary)] mb-2">
                        最近委托
                      </h3>
                      {stats && stats.recent.some((r) => r.role === selectedRole.name) ? (
                        <ul className="space-y-1.5">
                          {stats.recent
                            .filter((r) => r.role === selectedRole.name)
                            .slice(0, 8)
                            .map((r, i) => (
                              <li
                                key={`${r.ts}-${i}`}
                                className="flex items-center gap-2 text-xs text-[var(--color-text-secondary)]"
                              >
                                <span
                                  className={`shrink-0 inline-block px-1.5 py-0.5 text-[10px] rounded ${r.success ? 'bg-emerald-50 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300' : 'bg-red-50 text-red-700 dark:bg-red-900/40 dark:text-red-300'}`}
                                >
                                  {r.success ? '成功' : '失败'}
                                </span>
                                <span className="min-w-0 flex-1 truncate" title={r.task}>
                                  {r.task}
                                </span>
                                <span className="shrink-0 text-[var(--color-text-tertiary)]">
                                  {formatTimestamp(r.ts)}
                                </span>
                                <span className="shrink-0 text-[var(--color-text-tertiary)]">
                                  {r.duration_ms}ms
                                </span>
                                <span className="shrink-0 text-[var(--color-text-tertiary)]">
                                  {r.tokens}t
                                </span>
                              </li>
                            ))}
                        </ul>
                      ) : (
                        <p className="text-xs text-[var(--color-text-tertiary)]">暂无委托记录</p>
                      )}
                    </section>
                    <section>
                      <h3 className="text-sm font-semibold text-[var(--color-text-primary)] mb-2">
                        工具白名单
                      </h3>
                      {detail?.tools && detail.tools.length > 0 ? (
                        <div className="flex flex-wrap gap-1.5">
                          {detail.tools.map((t) => (
                            <span
                              key={t}
                              className="inline-block px-2 py-0.5 text-xs rounded bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-secondary)] font-mono"
                            >
                              {t}
                            </span>
                          ))}
                        </div>
                      ) : (
                        <p className="text-xs text-[var(--color-text-tertiary)]">
                          不限制（与主 Agent 相同）
                        </p>
                      )}
                    </section>
                  </>
                )}
              </div>
            </>
          )}
        </div>
      </div>

      {/* 破坏性操作确认（统一 ConfirmDialog 原语） */}
      <ConfirmDialog
        open={confirm !== null}
        title={confirm?.title ?? ''}
        message={confirm?.message ?? ''}
        danger
        confirmLabel={confirm?.action === 'delete' ? '删除' : '回退'}
        busy={actionPending}
        onConfirm={() => void (confirm?.action === 'delete' ? executeDelete() : executeReset())}
        onCancel={() => setConfirm(null)}
      />
    </div>
  );
}
