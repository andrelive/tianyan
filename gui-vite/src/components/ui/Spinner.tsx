import { Loader2 } from 'lucide-react';

/** 居中加载指示（原各面板手写 ≥12 处同构块；此原语统一尺寸/语义）。 */
export function Spinner({
  label = '加载中',
  className = 'py-16',
}: {
  label?: string;
  className?: string;
}) {
  return (
    <div
      className={`flex items-center justify-center ${className}`}
      role="status"
      aria-label={label}
    >
      <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
    </div>
  );
}
