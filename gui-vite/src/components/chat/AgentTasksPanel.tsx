import { useCallback, useEffect, useRef, useState } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { useUnifiedEvents, type UnifiedEvent } from '@/hooks/use-unified-events';
import { cancelTask, fetchSessionMessages, fetchTasks } from '@/lib/api-client';
import { getApiBase } from '@/lib/api-base';
import type {
  BackgroundTask,
  ChatMessage,
  MessageSegment,
  ToolCallEvent,
  ToolCallWithResult,
  ToolResultEvent,
} from '@/lib/types';
import { SegmentBlocks } from './MessageSegments';
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  CircleX,
  Layers,
  Loader2,
  MinusCircle,
  XCircle,
} from 'lucide-react';

/** 轮询间隔（后台任务状态变化）。 */
const POLL_INTERVAL_MS = 3000;

/** 子智能体消息流事件（ChatStreamEvent 同构 + task_id；ADR-026）。 */
interface TaskStreamEvent {
  task_id?: string;
  chunk_type: 'answer' | 'thought' | 'tool_call' | 'observation' | 'error' | 'message';
  delta?: string;
  thinking?: string | null;
  tool_call?: ToolCallEvent | null;
  tool_result?: ToolResultEvent | null;
}

/** 事件流 → 段渲染结构（与主对话流同构：thinking/text/tool + 结果挂卡）。 */
function eventsToBlocks(
  events: TaskStreamEvent[],
): { segments: MessageSegment[]; toolCalls: ToolCallWithResult[] } {
  const segments: MessageSegment[] = [];
  const toolCalls: ToolCallWithResult[] = [];
  for (const ev of events) {
    if (ev.chunk_type === 'thought' && ev.thinking) {
      segments.push({ type: 'thinking', text: ev.thinking });
    } else if (ev.chunk_type === 'answer' && ev.delta) {
      segments.push({ type: 'text', text: ev.delta });
    } else if (ev.chunk_type === 'tool_call' && ev.tool_call) {
      segments.push({ type: 'tool', tool_call: ev.tool_call });
      toolCalls.push({ ...ev.tool_call, result: null });
    } else if (ev.chunk_type === 'observation' && ev.tool_result) {
      const tc = toolCalls.find((c) => c.id === ev.tool_result!.tool_call_id);
      if (tc) {
        tc.result = ev.tool_result.content ?? null;
        tc.success = ev.tool_result.success;
        tc.duration_ms = ev.tool_result.duration_ms;
        tc.error = ev.tool_result.error;
      }
    }
  }
  return { segments, toolCalls };
}

/**
 * 会话后台任务面板（ADR-026，DSH 式右侧 dock）：展示当前会话发起的
 * 后台任务（delegate 委托 / command 终端命令），活跃在上、完成沉底。
 *
 * - 委托任务展开：子智能体消息流（历史 + SSE 实时增量，复用主对话流渲染）；
 * - 终端任务展开：输出尾部（output_tail）+ 状态/错误；
 * - 运行中可取消；终态保留展示（回看做了什么、结果如何）。
 * 无任何任务时整条不渲染。
 */
export default function AgentTasksPanel({ sessionId }: { sessionId: string | null }) {
  const [tasks, setTasks] = useState<BackgroundTask[]>([]);
  const [cancellingId, setCancellingId] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(false);
  const [openId, setOpenId] = useState<string | null>(null);
  /** 委托任务历史消息（task_id → 会话消息；展开时加载）。 */
  const [histories, setHistories] = useState<Record<string, ChatMessage[]>>({});
  /** 委托任务实时事件（task_id → SSE 增量；按到达顺序累积）。 */
  const [liveEvents, setLiveEvents] = useState<Record<string, TaskStreamEvent[]>>({});
  const liveRef = useRef<Record<string, TaskStreamEvent[]>>({});

  const poll = useCallback(async () => {
    try {
      const data = await fetchTasks();
      setTasks(data);
    } catch {
      /* 轮询失败静默保留旧数据 */
    }
  }, []);
  usePolling(poll, POLL_INTERVAL_MS, { enabled: !!sessionId });

  // ADR-028：统一事件订阅——task_status 实时刷新任务状态（终态/运行中），
  // command_output 累积终端输出增量（实时视图）。消息类事件由 hook 自动
  // 合并进 store，本面板只消费任务类事件。
  const [terminalOutputs, setTerminalOutputs] = useState<Record<string, string>>({});
  const terminalRef = useRef<Record<string, string>>({});
  useUnifiedEvents((ev: UnifiedEvent) => {
    if (ev.type === 'task_status' && ev.task_id) {
      // 状态变化 → 拉取权威列表（终态/运行中实时更新）
      void poll();
    } else if (ev.type === 'command_output' && ev.task_id && ev.delta) {
      const prev = terminalRef.current[ev.task_id] ?? '';
      // 输出尾部上限（与后端 OUTPUT_TAIL_MAX_BYTES 对齐，防无限增长）
      const next = (prev + ev.delta).slice(-64 * 1024);
      terminalRef.current[ev.task_id] = next;
      setTerminalOutputs({ ...terminalRef.current });
    }
  });

  // SSE 订阅：子智能体消息流（ADR-026；事件按 task_id 归集）
  useEffect(() => {
    if (!sessionId) return;
    if (typeof EventSource === 'undefined') return; // 测试环境（jsdom）无 EventSource
    const es = new EventSource(getApiBase() + '/tasks/stream');
    es.onmessage = (e) => {
      try {
        const ev = JSON.parse(e.data) as TaskStreamEvent;
        if (!ev.task_id) return;
        const list = liveRef.current[ev.task_id] ?? [];
        list.push(ev);
        liveRef.current[ev.task_id] = list;
        setLiveEvents({ ...liveRef.current });
      } catch {
        /* 畸形事件跳过 */
      }
    };
    return () => es.close();
  }, [sessionId]);

  // 展开委托任务时加载历史消息
  const handleOpen = useCallback(
    async (task: BackgroundTask) => {
      const next = openId === task.id ? null : task.id;
      setOpenId(next);
      if (next && task.kind === 'delegate' && !histories[task.id]) {
        try {
          const resp = await fetchSessionMessages(task.id);
          setHistories((h) => ({ ...h, [task.id]: resp.messages }));
        } catch {
          /* 历史加载失败：仅显示实时增量 */
        }
      }
    },
    [openId, histories],
  );

  const handleCancel = useCallback(
    async (taskId: string) => {
      setCancellingId(taskId);
      try {
        await cancelTask(taskId);
        await poll();
      } catch {
        /* 取消失败（如任务已结束）：下次轮询自然更新 */
      } finally {
        setCancellingId(null);
      }
    },
    [poll],
  );

  // 本会话任务，按创建时间排序（委托/命令两套注册表 seq 独立，统一按时间线）
  const mine = tasks
    .filter((t) => t.parent_session_id === sessionId)
    .sort((a, b) => a.created_at - b.created_at || a.seq - b.seq);
  if (!sessionId || mine.length === 0) return null;

  const running = mine.filter((t) => t.status === 'pending' || t.status === 'running');
  const done = mine.filter((t) => !(t.status === 'pending' || t.status === 'running'));

  const statusLabel = (t: BackgroundTask) => {
    if (t.status === 'pending') return '等待中';
    if (t.status === 'completed') return '已完成';
    if (t.status === 'failed') return t.error ? '失败（' + t.error + '）' : '失败';
    if (t.status === 'cancelled') return '已取消';
    return null;
  };

  const renderTask = (t: BackgroundTask) => {
    const terminal = t.status === 'completed' || t.status === 'failed' || t.status === 'cancelled';
    const open = openId === t.id;
    const label = statusLabel(t);
    const live = liveEvents[t.id] ?? [];
    const liveBlocks = eventsToBlocks(live);
    const history = histories[t.id] ?? [];
    return (
      <div
        key={t.id}
        className={
          'border rounded-lg overflow-hidden bg-[var(--color-bg-primary)]' +
          (t.status === 'failed'
            ? ' border-[var(--color-error)]'
            : ' border-[var(--color-border)]')
        }
      >
        <button
          type="button"
          onClick={() => void handleOpen(t)}
          aria-expanded={open}
          className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left hover:bg-[var(--color-bg-hover)] transition-colors"
        >
          <span className="shrink-0 text-[var(--color-text-tertiary)]">
            {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </span>
          <span className="shrink-0 flex">
            {t.status === 'running' ? (
              <Loader2 size={12} className="animate-spin text-blue-500" />
            ) : t.status === 'pending' ? (
              <Loader2 size={12} className="text-[var(--color-text-tertiary)]" />
            ) : t.status === 'completed' ? (
              <CircleCheck size={12} className="text-[var(--color-success)]" />
            ) : t.status === 'failed' ? (
              <CircleX size={12} className="text-[var(--color-error)]" />
            ) : (
              <MinusCircle size={12} className="text-[var(--color-text-tertiary)]" />
            )}
          </span>
          <span
            className={
              'shrink-0 px-1 rounded text-[10px] border ' +
              (t.kind === 'command'
                ? 'border-[var(--color-border)] text-[var(--color-text-tertiary)]'
                : 'border-[var(--color-accent)] text-[var(--color-accent)]')
            }
          >
            {t.kind === 'command' ? '终端' : '委托'}
          </span>
          <span className="min-w-0 flex-1 truncate text-xs font-medium text-[var(--color-text-primary)]">
            {t.description || t.id}
          </span>
          {label && (
            <span
              className={
                'shrink-0 text-xs ' +
                (t.status === 'failed'
                  ? 'text-[var(--color-error)]'
                  : 'text-[var(--color-text-tertiary)]')
              }
            >
              {label}
            </span>
          )}
          {!terminal && (
            <span
              role="button"
              tabIndex={0}
              aria-label={'取消后台任务 ' + (t.description || t.id)}
              onClick={(e) => {
                e.stopPropagation();
                void handleCancel(t.id);
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.stopPropagation();
                  void handleCancel(t.id);
                }
              }}
              className="shrink-0 px-1.5 py-0.5 rounded border border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:text-[var(--color-error)] hover:border-[var(--color-error)] disabled:opacity-50 text-xs cursor-pointer"
            >
              {cancellingId === t.id ? (
                <Loader2 size={11} className="animate-spin" />
              ) : (
                <XCircle size={11} />
              )}
              取消
            </span>
          )}
        </button>

        {open && (
          <div className="border-t border-[var(--color-border)] px-2.5 py-2 space-y-2">
            {t.kind === 'delegate' ? (
              <>
                {/* 历史消息（会话存储加载） */}
                {history.map((m, i) => (
                  <div key={m.id ?? i} className="space-y-1">
                    {m.segments && m.segments.length > 0 && (
                      <SegmentBlocks segments={m.segments} toolCalls={m.tool_calls} isUser={false} />
                    )}
                  </div>
                ))}
                {/* 实时增量（SSE） */}
                {liveBlocks.segments.length > 0 && (
                  <SegmentBlocks segments={liveBlocks.segments} toolCalls={liveBlocks.toolCalls} isUser={false} />
                )}
                {liveBlocks.segments.length === 0 && history.length === 0 && (
                  <p className="text-xs text-[var(--color-text-tertiary)]">
                    {t.status === 'running' || t.status === 'pending'
                      ? '子智能体工作中…'
                      : '无过程记录'}
                  </p>
                )}
              </>
            ) : (
              <>
                {/* 终端输出：实时增量（command_output 事件累积）优先，
                    尾部快照（output_tail）兜底——实时流尽力而为，
                    完整输出经日志文件分页查看（ADR-028） */}
                {(terminalOutputs[t.id] || t.output_tail) && (
                  <pre className="max-h-48 overflow-auto px-2.5 py-2 rounded text-[11px] leading-relaxed whitespace-pre-wrap break-words font-mono bg-[#0d1117] text-[#c9d1d9]">
                    {terminalOutputs[t.id] || t.output_tail}
                  </pre>
                )}
                {t.result && (
                  <p className="text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap break-words">
                    {t.result}
                  </p>
                )}
                {t.error && (
                  <p className="text-xs text-[var(--color-error)] whitespace-pre-wrap break-words">
                    {t.error}
                  </p>
                )}
              </>
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="shrink-0 w-72 border-l border-[var(--color-border)] bg-[var(--color-bg-secondary)] flex flex-col overflow-hidden">
      {!collapsed && (
        <>
          <div className="flex items-center gap-1.5 px-3 py-2 border-b border-[var(--color-border)] text-xs font-medium text-[var(--color-text-secondary)] shrink-0">
            <Layers size={12} />
            <span>后台任务</span>
            {running.length > 0 && (
              <span className="text-[var(--color-text-tertiary)]">运行中 {running.length}</span>
            )}
            {done.length > 0 && (
              <span className="text-[var(--color-text-tertiary)]">已结束 {done.length}</span>
            )}
            <span className="flex-1" />
            <button
              type="button"
              onClick={() => setCollapsed(true)}
              aria-label="收起任务面板"
              className="text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)]"
            >
              <ChevronRight size={12} />
            </button>
          </div>
          <div className="flex-1 overflow-y-auto px-2.5 py-2 space-y-1.5">
            {running.map(renderTask)}
            {done.length > 0 && (
              <p className="flex items-center gap-1.5 pt-2 pb-0.5 text-xs font-medium text-[var(--color-text-tertiary)]">
                <span>已结束</span>
                <span className="flex-1 border-t border-[var(--color-border)]" />
              </p>
            )}
            {done.map(renderTask)}
          </div>
        </>
      )}
      {collapsed && (
        <button
          type="button"
          onClick={() => setCollapsed(false)}
          aria-label="展开任务面板"
          className="flex-1 flex items-center justify-center text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)]"
        >
          <ChevronLeft size={12} />
        </button>
      )}
    </div>
  );
}
