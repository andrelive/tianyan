import { useState } from 'react';
import { cn } from '@/lib/utils';
import { ChevronDown, ChevronRight, CheckCircle2, XCircle, Clock } from 'lucide-react';
import type { SkillCallInfo } from '@/lib/types';

interface Props {
  info: SkillCallInfo;
}

export default function SkillCallCard({ info }: Props) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div
      className={cn(
        'rounded-lg border-l-4 p-3 my-2 text-lg',
        info.success
          ? 'border-l-green-500 bg-green-50 dark:bg-green-950/30'
          : 'border-l-red-500 bg-red-50 dark:bg-red-950/30',
      )}
    >
      <button
        onClick={() => setExpanded(!expanded)}
        aria-expanded={expanded}
        aria-label={`${info.skill_name}，${expanded ? '收起详情' : '展开详情'}`}
        className="flex items-center justify-between w-full gap-2"
      >
        <div className="flex items-center gap-2 min-w-0">
          {info.success ? (
            <CheckCircle2 className="w-4 h-4 shrink-0 text-green-600 dark:text-green-400" />
          ) : (
            <XCircle className="w-4 h-4 shrink-0 text-red-600 dark:text-red-400" />
          )}
          <span className="font-medium truncate text-[var(--color-text-primary)]">
            {info.skill_name}
          </span>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <span className="flex items-center gap-1 text-base text-[var(--color-text-tertiary)]">
            <Clock className="w-3 h-3" />
            {info.execution_time_ms}ms
          </span>
          {expanded ? (
            <ChevronDown className="w-4 h-4 text-[var(--color-text-tertiary)]" />
          ) : (
            <ChevronRight className="w-4 h-4 text-[var(--color-text-tertiary)]" />
          )}
        </div>
      </button>

      {expanded && (
        <div className="mt-2 pt-2 border-t border-[var(--color-border)] space-y-1">
          <p className="text-base text-[var(--color-text-secondary)]">
            <span className="font-medium">技能 ID:</span> {info.skill_id}
          </p>
          {info.error && (
            <p className="text-base text-red-600 dark:text-red-400">
              <span className="font-medium">错误:</span> {info.error}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
