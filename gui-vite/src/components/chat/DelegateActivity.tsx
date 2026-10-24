import { useEffect, useMemo, useRef, useState } from 'react';
import { AlignLeft, Brain, ChevronDown, ChevronRight, Loader2, XCircle } from 'lucide-react';
import { useAppStore } from '@/lib/store';
import { TOOL_PRESENTATION_LABELS, type ChatMessage, type ToolCallWithResult } from '@/lib/types';
import { firstNonEmptyLine, lastNonEmptyLine } from '@/lib/utils';
import {
  formatArguments,
  formatDuration,
  presentationIcon,
  summarizeToolTarget,
} from '@/lib/tool-presentation';

/**
 * 子智能体**活动流**（activity feed）——窄列（侧栏 ~290px）里的过程展示。
 *
 * ## 设计约束（改动前必读：这些是刻意为之，不是没做完）
 *
 * - **纯投影，不约束子智能体**。数据源就是它已产生的真实时间线
 *   (`StructuredMessage.segments` = thinking / text / tool)，按发生顺序一段一行。
 *   - **不做阶段归类**（不硬分「探索 / 实施 / 验证」）——一旦归类就等于假设它
 *     有那些阶段，那是协议约束，不是展示；
 *   - **不假设有工具调用**（零工具调用、纯思考、一连串失败都必须能显示）；
 *   - **不显示「3/7」式进度分数**——总数意味着「既定计划」，同样是隐性约束。
 *     要表达"做了多少"只说「已 N 步」，不说「N/M」。
 * - **无损**：每行都能展开看到原始内容（思考全文 / 正文全文 / 参数 + 结果），
 *   不是压缩信息换高度。
 * - 侧栏宽度是硬约束，所以渲染尺寸（11px 行高 ~22px）与主会话（16px）不同；
 *   这是**参数**不同，不是另一套语义。
 */

/** 空数组稳定引用（zustand 选择器返回新引用会触发 React #185 无限重渲染）。 */
const EMPTY_MESSAGES: ChatMessage[] = [];

/** 活动流一行：消息段的投影。 */
type ActivityRow =
  | { key: string; kind: 'thinking'; text: string }
  | { key: string; kind: 'text'; text: string }
  | { key: string; kind: 'tool'; call: ToolCallWithResult };

/**
 * 把会话消息摊平成活动行。
 *
 * 工具结果不在段里——它按 `tool_call_id` 挂在消息的 `tool_calls` 上
 * （`applyToolResult` 更新那里），所以先建 id → 调用 的索引再投影。
 * 相邻同类文本段合并（与主会话 `SegmentBlocks` 同策略）：流式逐 delta
 * 到达会产生大量碎片段，不合并会一行一个 token。
 */
function toActivityRows(messages: ChatMessage[]): ActivityRow[] {
  const byId = new Map<string, ToolCallWithResult>();
  for (const message of messages) {
    for (const call of message.tool_calls ?? []) {
      if (call.id) byId.set(call.id, call);
    }
  }

  const rows: ActivityRow[] = [];
  messages.forEach((message, mi) => {
    (message.segments ?? []).forEach((seg, si) => {
      const key = `${message.id ?? `#${mi}`}:${si}`;
      if (seg.type === 'tool') {
        const id = seg.tool_call.id ?? '';
        rows.push({ key, kind: 'tool', call: byId.get(id) ?? seg.tool_call });
        return;
      }
      if (seg.type === 'thinking') {
        const prev = rows[rows.length - 1];
        if (prev && prev.kind === 'thinking') {
          prev.text += seg.text;
          return;
        }
        rows.push({ key, kind: 'thinking', text: seg.text });
        return;
      }
      const prev = rows[rows.length - 1];
      if (prev && prev.kind === 'text') {
        prev.text += seg.text;
        return;
      }
      rows.push({ key, kind: 'text', text: seg.text });
    });
  });
  return rows;
}

/** 展开行外壳（三类行共用：收起态一行，展开态一块原文）。 */
function RowShell({
  open,
  onToggle,
  icon,
  label,
  summary,
  meta,
  children,
}: {
  open: boolean;
  onToggle: () => void;
  icon: React.ReactNode;
  label: string;
  summary: string;
  meta?: string;
  children?: React.ReactNode;
}) {
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="w-full flex items-center gap-1.5 px-1 py-[3px] rounded text-[11px] leading-5 text-left hover:bg-[var(--color-bg-hover)] transition-colors"
      >
        <span className="shrink-0 flex">{icon}</span>
        <span className="shrink-0 text-[var(--color-text-secondary)]">{label}</span>
        {summary !== '' && (
          <span
            className="min-w-0 flex-1 truncate text-[var(--color-text-tertiary)]"
            title={summary}
          >
            {summary}
          </span>
        )}
        {summary === '' && <span className="flex-1" />}
        {meta && (
          <span className="shrink-0 text-[10px] text-[var(--color-text-tertiary)]">{meta}</span>
        )}
        <span className="shrink-0 text-[var(--color-text-tertiary)]">
          {open ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
        </span>
      </button>
      {open && children}
    </div>
  );
}

/** 展开区容器（参数 / 失败原因 / 结果共用样式）。 */
function DetailPre({ text, tone }: { text: string; tone?: 'error' }) {
  return (
    <pre
      className={
        'mb-1 mx-1 px-1.5 py-1 rounded text-[10px] leading-4 font-mono whitespace-pre-wrap ' +
        'break-words max-h-48 overflow-auto ' +
        (tone === 'error'
          ? 'text-[var(--color-error)] bg-[var(--color-error)]/5'
          : 'text-[var(--color-text-secondary)] bg-[var(--color-bg-hover)]/40')
      }
    >
      {text}
    </pre>
  );
}

/**
 * 子智能体的活动流（窄列过程展示）。无消息时显示占位文案，不抛错。
 */
export default function DelegateActivity({ taskId, status }: { taskId: string; status: string }) {
  const messages = useAppStore((s) => s.sessionMessages[taskId] ?? EMPTY_MESSAGES);
  const rows = useMemo(() => toActivityRows(messages), [messages]);
  const [openKey, setOpenKey] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const live = status === 'running' || status === 'pending';

  // 运行中贴底（新动作出现即可见）；已结束停在顶部（从第一步读起）
  useEffect(() => {
    if (!live) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [rows, live]);

  if (rows.length === 0) {
    return (
      <p className="px-1 py-1 text-[11px] text-[var(--color-text-tertiary)]">
        {live ? '子智能体工作中…' : '无过程记录'}
      </p>
    );
  }

  const toggle = (key: string) => setOpenKey((cur) => (cur === key ? null : key));

  return (
    <div ref={scrollRef} className="max-h-[45vh] overflow-y-auto overflow-x-hidden">
      {rows.map((row) => {
        const open = openKey === row.key;

        if (row.kind === 'tool') {
          const call = row.call;
          const Icon = presentationIcon(call.presentation);
          const label = TOOL_PRESENTATION_LABELS[call.presentation] ?? call.name;
          const target = summarizeToolTarget(call.arguments);
          const failed = call.success === false || (call.error != null && call.error !== '');
          const settled = call.success !== undefined || call.duration_ms !== undefined;
          return (
            <RowShell
              key={row.key}
              open={open}
              onToggle={() => toggle(row.key)}
              icon={
                failed ? (
                  <XCircle size={11} className="text-[var(--color-error)]" />
                ) : settled ? (
                  <Icon size={11} className="text-[var(--color-text-tertiary)]" />
                ) : (
                  <Loader2 size={11} className="animate-spin text-blue-500" />
                )
              }
              label={label}
              summary={target}
              meta={call.duration_ms !== undefined ? formatDuration(call.duration_ms) : undefined}
            >
              <DetailPre text={formatArguments(call.arguments)} />
              {call.error && <DetailPre text={call.error} tone="error" />}
              {call.result != null && call.result !== '' && <DetailPre text={call.result} />}
            </RowShell>
          );
        }

        const isThinking = row.kind === 'thinking';
        const banner = isThinking ? lastNonEmptyLine(row.text) : firstNonEmptyLine(row.text);
        return (
          <RowShell
            key={row.key}
            open={open}
            onToggle={() => toggle(row.key)}
            icon={
              isThinking ? (
                <Brain size={11} className="text-[var(--color-text-tertiary)]" />
              ) : (
                <AlignLeft size={11} className="text-[var(--color-text-tertiary)]" />
              )
            }
            label={isThinking ? '思考' : '输出'}
            summary={banner}
            meta={`${row.text.length} 字`}
          >
            <DetailPre text={row.text} />
          </RowShell>
        );
      })}
    </div>
  );
}

/**
 * 列表行上的「当前在做什么」（任务卡片头部一行）。
 *
 * 只在活动任务上渲染（已完成的不需要，也省掉无谓的 store 计算）。
 * 选择器**只返回字符串**（原始值，引用天然稳定——避免 React #185）。
 * 刻意不返回「第 3/7 步」：总数意味着既定计划（见文件头的设计约束）。
 */
export function TaskLiveStep({ taskId }: { taskId: string }) {
  const step = useAppStore((s) => {
    const messages = s.sessionMessages[taskId];
    if (!messages || messages.length === 0) return '';
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      const calls = messages[i].tool_calls ?? [];
      const lastCall = calls[calls.length - 1];
      if (lastCall) {
        const label = TOOL_PRESENTATION_LABELS[lastCall.presentation] ?? lastCall.name;
        const target = summarizeToolTarget(lastCall.arguments);
        return target !== '' ? `${label} ${target}` : label;
      }
      const segments = messages[i].segments ?? [];
      const lastSeg = segments[segments.length - 1];
      if (lastSeg?.type === 'thinking') return '思考中…';
      if (lastSeg?.type === 'text' && lastSeg.text.trim() !== '') return '输出中…';
    }
    return '';
  });
  if (step === '') return null;
  return (
    <span
      className="mt-0.5 block truncate text-[10px] text-[var(--color-text-tertiary)]"
      title={step}
    >
      {step}
    </span>
  );
}
