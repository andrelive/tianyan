/**
 * 工具展示的**共享口径**——主会话的工具卡片与子智能体活动流共用同一套映射。
 *
 * 展示意图（`presentation`）由后端在工具注册时声明，前端只做投影：图标、标签、
 * 目标摘要、耗时格式都从这里取。两份渲染各自维护一套是口径漂移的经典来源
 * （同一工具在主区叫「编辑文件」、在侧栏叫「工具」）。
 */

import type { LucideIcon } from 'lucide-react';
import {
  Brain,
  Code2,
  FilePlus2,
  FileText,
  GitBranch,
  Globe,
  Image as ImageIcon,
  PenLine,
  Search,
  Terminal,
  Wrench,
} from 'lucide-react';

/** 展示意图 → 图标（未登记的类型回退通用扳手）。 */
export const PRESENTATION_ICONS: Record<string, LucideIcon> = {
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
  image: ImageIcon,
};

/** 按展示意图取图标（未登记的类型回退通用扳手）。 */
export function presentationIcon(presentation: string): LucideIcon {
  return PRESENTATION_ICONS[presentation] ?? Wrench;
}

/** 毫秒 → 人类可读耗时（≥1s 用秒，保留两位）。 */
export function formatDuration(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms}ms`;
}

/** 参数 JSON 美化（非法 JSON 时原样返回——宁可显示原始也不吞掉内容）。 */
export function formatArguments(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

/** 常见「目标」字段，按优先级取第一个非空字符串。 */
const TARGET_FIELDS = [
  'path',
  'file_path',
  'filePath',
  'pattern',
  'query',
  'command',
  'cmd',
  'url',
  'title',
  'task',
  'goal',
  'description',
  'session_id',
  'name',
];

/** patch 类信封里的目标文件（apply_patch 的参数是 diff 文本而非 JSON）。 */
const PATCH_FILE_RE = /\*\*\*\s+(?:Update|Add|Delete)\s+File:\s*([^\n]+)/;

const MAX_TARGET_CHARS = 80;

/**
 * 从工具参数里提取「一句话目标」（窄列单行展示用）。
 *
 * 只挑常见字段、**不做语义推断**：猜错比截断更误导，取不到就把原始参数截断。
 */
export function summarizeToolTarget(raw: string): string {
  const patchMatch = PATCH_FILE_RE.exec(raw);
  if (patchMatch) return clip(patchMatch[1].trim());

  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      const obj = parsed as Record<string, unknown>;
      for (const field of TARGET_FIELDS) {
        const value = obj[field];
        if (typeof value === 'string' && value.trim() !== '') return clip(value.trim());
      }
      // 兜底：第一个非空字符串字段
      for (const value of Object.values(obj)) {
        if (typeof value === 'string' && value.trim() !== '') return clip(value.trim());
      }
      return '';
    }
    if (typeof parsed === 'string') return clip(parsed.trim());
  } catch {
    /* 非 JSON（如纯文本参数）：落到下面的截断 */
  }
  return clip(raw.trim());
}

function clip(text: string): string {
  const oneLine = text.replace(/\s+/g, ' ').trim();
  return oneLine.length > MAX_TARGET_CHARS ? `${oneLine.slice(0, MAX_TARGET_CHARS)}…` : oneLine;
}
