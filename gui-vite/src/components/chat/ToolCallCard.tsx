import { useState, memo, type ComponentType } from 'react';
import {
  ChevronDown,
  ChevronRight,
  Terminal,
  FileText,
  Search,
  Globe,
  PenLine,
  FilePlus2,
  Brain,
  GitBranch,
  Code2,
  Wrench,
  CheckCircle2,
  XCircle,
  Clock,
  Loader2,
} from 'lucide-react';
import type { ToolCallEvent } from '@/lib/types';
import { TOOL_PRESENTATION_LABELS } from '@/lib/types';

/** 展示意图 → 图标 */
const PRESENTATION_ICONS: Record<string, ComponentType<{ className?: string }>> = {
  read: FileText,
  write: FilePlus2,
  terminal: Terminal,
  diff: PenLine,
  search: Search,
  web: Globe,
  skill: Brain,
  knowledge: FilePlus2,
  delegate: GitBranch,
  code: Code2,
};

/** 毫秒 → 人类可读耗时。 */
function formatDuration(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms}ms`;
}

interface Props {
  /** 工具调用事件（名称/参数/展示意图） */
  event: ToolCallEvent;
  /** 对应执行结果（完整内容；有值时在卡片内渲染可展开的结果区） */
  result?: string | null;
}

/**
 * 工具调用卡片（A2 展示契约）。
 *
 * 数据驱动：按 event.presentation 选择图标与标签，展开可查看参数 JSON。
 * 后端在 ToolRegistry 注册展示意图（内置工具一一对应），前端只做投影。
 */
function ToolCallCard({ event, result }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [resultOpen, setResultOpen] = useState(false);
  const Icon = PRESENTATION_ICONS[event.presentation] ?? Wrench;
  const label = TOOL_PRESENTATION_LABELS[event.presentation] ?? '工具';
  // 计时/成败元数据：observation 事件实时填充（流式）或后端透传（历史）
  const hasStatus = event.success !== undefined || event.duration_ms !== undefined;
  const failed = event.success === false || (event.error != null && event.error.length > 0);

  return (
    <div
      className={`border rounded-lg overflow-hidden ${
        failed
          ? 'border-red-500/50 bg-red-50/40 dark:bg-red-950/20'
          : 'border-[var(--color-border)]'
      }`}
    >
      {/* 卡片头：图标 + 标签 + 工具名 + 展开开关 */}
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left hover:bg-[var(--color-bg-hover)] transition-colors"
        aria-expanded={expanded}
        aria-label={`${label} ${event.name}`}
      >
        <span className="shrink-0">
          <Icon className="w-3.5 h-3.5 text-[var(--color-text-tertiary)]" />
        </span>
        <span className="text-base font-medium text-[var(--color-text-secondary)]">{label}</span>
        {event.name ? (
          <code className="text-base text-[var(--color-text-primary)] font-mono truncate">
            {event.name}
          </code>
        ) : (
          <span className="text-base text-[var(--color-text-primary)]">工具结果</span>
        )}
        {/* 运行中状态：结果未达、未失败、未成功 —— 显示 spinner（执行挂起可辨） */}
        {result == null && !failed && !event.success && (
          <span className="shrink-0 flex items-center gap-1.5 text-base text-[var(--color-text-tertiary)]">
            <Loader2 className="w-3.5 h-3.5 animate-spin" />
            <span>运行中</span>
          </span>
        )}
        {/* 状态区：失败红色 / 成功绿色标记 + 耗时（observation 到达后显示） */}
        {hasStatus && (
          <span className="shrink-0 flex items-center gap-2 text-base">
            {failed ? (
              <span className="flex items-center gap-1 text-red-600 dark:text-red-400">
                <XCircle className="w-3.5 h-3.5" />
                <span>失败</span>
              </span>
            ) : event.success === true ? (
              <span className="flex items-center gap-1 text-green-600 dark:text-green-400">
                <CheckCircle2 className="w-3.5 h-3.5" />
                <span>成功</span>
              </span>
            ) : null}
            {event.duration_ms !== undefined && (
              <span className="flex items-center gap-1 text-[var(--color-text-tertiary)]">
                <Clock className="w-3.5 h-3.5" />
                {formatDuration(event.duration_ms)}
              </span>
            )}
          </span>
        )}
        <span className="ml-auto shrink-0 flex items-center gap-1">
          {expanded ? (
            <ChevronDown className="w-3.5 h-3.5 text-[var(--color-text-tertiary)] rotate-180 transition-transform" />
          ) : (
            <ChevronDown className="w-3.5 h-3.5 text-[var(--color-text-tertiary)] transition-transform" />
          )}
        </span>
      </button>

      {/* 失败原因（失败时展示在参数区上方） */}
      {failed && event.error && (
        <div className="px-3 py-2 text-base text-red-600 dark:text-red-400 border-t border-red-500/30 bg-red-50/50 dark:bg-red-950/30 whitespace-pre-wrap break-words">
          {event.error}
        </div>
      )}

      {/* 展开区：参数 JSON */}
      {expanded && (
        <pre className="max-h-48 overflow-auto px-3 py-2 text-base font-mono text-[var(--color-text-secondary)] bg-[var(--color-bg-hover)]/40 whitespace-pre-wrap break-words">
          {formatArguments(event.arguments)}
        </pre>
      )}

      {/* 工具结果区：调用与结果合并渲染（完整内容，默认收起） */}
      {result != null && (
        <div className="border-t border-[var(--color-border)]">
          <button
            type="button"
            onClick={() => setResultOpen((v) => !v)}
            aria-expanded={resultOpen}
            className="w-full flex items-center gap-1.5 px-2.5 py-1.5 text-base text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)] transition-colors"
          >
            {resultOpen ? (
              <ChevronDown size={12} className="shrink-0" />
            ) : (
              <ChevronRight size={12} className="shrink-0" />
            )}
            <span>工具结果</span>
          </button>
          {resultOpen && (
            <pre className="max-h-96 overflow-auto px-3 pb-2 text-xs leading-relaxed whitespace-pre-wrap break-words font-mono text-[var(--color-text-secondary)]">
              {result}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

/** 参数 JSON 美化（非法 JSON 时原样显示） */
function formatArguments(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

export default memo(ToolCallCard);
