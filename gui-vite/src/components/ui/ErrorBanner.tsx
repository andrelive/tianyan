import { AlertCircle } from 'lucide-react';

/**
 * 错误横幅（此前各面板 bg-red-50 块手写 ≥10 处；此一处统一样式 + 可选重试）。
 * 语义：`role="alert"` 供屏幕阅读器即时播报。
 */
export function ErrorBanner({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div
      role="alert"
      className="flex items-center justify-between gap-3 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
    >
      <span className="flex items-center gap-2">
        <AlertCircle size={16} className="shrink-0" />
        <span>{message}</span>
      </span>
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="shrink-0 px-2 py-1 text-xs rounded border border-red-300 dark:border-red-700 hover:bg-red-100 dark:hover:bg-red-900/40"
        >
          重试
        </button>
      )}
    </div>
  );
}
