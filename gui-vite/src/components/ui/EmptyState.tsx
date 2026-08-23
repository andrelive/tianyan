import type { LucideIcon } from 'lucide-react';

/** 空态（此前各面板图标 + 文案手写 ≥8 处；统一图标透明度/间距语义）。 */
export function EmptyState({
  icon: Icon,
  title,
  hint,
}: {
  icon: LucideIcon;
  title: string;
  hint?: string;
}) {
  return (
    <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
      <Icon size={40} className="mb-3 opacity-40" />
      <p className="text-sm">{title}</p>
      {hint && <p className="text-xs mt-2 opacity-70 max-w-md text-center">{hint}</p>}
    </div>
  );
}
