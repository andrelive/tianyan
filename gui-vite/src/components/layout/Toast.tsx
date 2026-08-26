import { useEffect } from 'react';
import { useAppStore } from '@/lib/store';
import type { ToastMessage } from '@/lib/types';

/** 单条 Toast：3s 自动消失 + 点击关闭（每条独立计时，支持多条堆叠）。 */
function ToastItem({
  toast,
  onClose,
}: {
  toast: ToastMessage & { id: string };
  onClose: () => void;
}) {
  useEffect(() => {
    const timer = setTimeout(onClose, 3000);
    return () => clearTimeout(timer);
  }, [onClose]);

  const bgColor =
    toast.type === 'error'
      ? 'bg-red-50 border-red-200 text-red-800 dark:bg-red-950 dark:border-red-800 dark:text-red-200'
      : toast.type === 'success'
        ? 'bg-green-50 border-green-200 text-green-800 dark:bg-green-950 dark:border-green-800 dark:text-green-200'
        : 'bg-blue-50 border-blue-200 text-blue-800 dark:bg-blue-950 dark:border-blue-800 dark:text-blue-200';

  return (
    <div
      role="alert"
      aria-live="assertive"
      className={`flex items-center gap-2 px-4 py-3 rounded-lg border shadow-lg cursor-pointer ${bgColor} animate-in fade-in slide-in-from-top-2`}
      onClick={onClose}
    >
      <span className="text-sm">{toast.message}</span>
      <button
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        className="ml-2 opacity-60 hover:opacity-100"
        aria-label="关闭通知"
      >
        ✕
      </button>
    </div>
  );
}

/** Toast 容器：渲染全部堆叠的 toast（右上角纵向排列）。 */
export default function Toast() {
  const toasts = useAppStore((s) => s.toasts);
  const hideToast = useAppStore((s) => s.hideToast);

  if (toasts.length === 0) return null;

  return (
    <div className="fixed top-4 right-4 z-50 flex flex-col gap-2">
      {toasts.map((t) => (
        <ToastItem key={t.id} toast={t} onClose={() => hideToast(t.id)} />
      ))}
    </div>
  );
}
