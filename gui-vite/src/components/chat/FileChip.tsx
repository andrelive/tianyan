import { X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { fileIconFor, type FileRef } from '@/lib/attachments';

interface Props {
  /** 文件引用（只展示文件名；完整路径只进 `title`，不占 chip 文本）。 */
  file: FileRef;
  /** 提供则渲染 `×` 删除按钮（输入区待发 chip）；缺省 = 只读（消息区回显）。 */
  onRemove?: () => void;
  className?: string;
}

/**
 * 文件引用 chip：图标 + 文件名（截断）+ 可选删除按钮。
 *
 * 输入区（可删）与消息区（只读）共用同一个组件——两处视觉必须一致，否则
 * 「发出去的文件」与「待发的文件」看起来像两种东西。
 */
export default function FileChip({ file, onRemove, className }: Props) {
  const { Icon, className: iconClass } = fileIconFor(file.name);
  return (
    <span
      className={cn(
        'inline-flex max-w-[16rem] items-center gap-1.5 rounded-lg px-2 py-1',
        'border border-[var(--color-border)] bg-[var(--color-bg-tertiary)]',
        'text-xs text-[var(--color-text-secondary)]',
        className,
      )}
      // 完整路径只在悬停提示里出现：chip 文本保持短（长路径会把输入框撑开）
      title={file.path}
      data-testid="file-chip"
    >
      <Icon className={cn('w-3.5 h-3.5 shrink-0', iconClass)} aria-hidden />
      <span className="truncate">{file.name}</span>
      {onRemove && (
        <button
          type="button"
          onClick={onRemove}
          className="shrink-0 p-0.5 rounded-full text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors"
          aria-label={`移除文件 ${file.name}`}
          title={`移除 ${file.name}`}
        >
          <X className="w-3 h-3" />
        </button>
      )}
    </span>
  );
}
