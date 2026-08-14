import { useState, memo, type ComponentType } from 'react';
import {
  ChevronDown,
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

interface Props {
  /** 工具调用事件（名称/参数/展示意图） */
  event: ToolCallEvent;
}

/**
 * 工具调用卡片（A2 展示契约）。
 *
 * 数据驱动：按 event.presentation 选择图标与标签，展开可查看参数 JSON。
 * 后端在 ToolRegistry 注册展示意图（内置工具一一对应），前端只做投影。
 */
function ToolCallCard({ event }: Props) {
  const [expanded, setExpanded] = useState(false);
  const Icon = PRESENTATION_ICONS[event.presentation] ?? Wrench;
  const label = TOOL_PRESENTATION_LABELS[event.presentation] ?? '工具';

  return (
    <div className="border border-[var(--color-border)] rounded-lg overflow-hidden">
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
        <span className="text-xs font-medium text-[var(--color-text-secondary)]">{label}</span>
        <code className="text-xs text-[var(--color-text-primary)] font-mono truncate">
          {event.name}
        </code>
        <span className="ml-auto shrink-0 flex items-center gap-1">
          {expanded ? (
            <ChevronDown className="w-3.5 h-3.5 text-[var(--color-text-tertiary)] rotate-180 transition-transform" />
          ) : (
            <ChevronDown className="w-3.5 h-3.5 text-[var(--color-text-tertiary)] transition-transform" />
          )}
        </span>
      </button>

      {/* 展开区：参数 JSON */}
      {expanded && (
        <pre className="max-h-48 overflow-auto px-3 py-2 text-xs font-mono text-[var(--color-text-secondary)] bg-[var(--color-bg-hover)]/40 whitespace-pre-wrap break-words">
          {formatArguments(event.arguments)}
        </pre>
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
