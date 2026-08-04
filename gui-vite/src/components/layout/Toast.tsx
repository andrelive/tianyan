import { useEffect } from 'react';
import { useAppStore } from '@/lib/store';

export default function Toast() {
  const toast = useAppStore((s) => s.toast);
  const hideToast = useAppStore((s) => s.hideToast);

  useEffect(() => {
    if (toast) {
      const timer = setTimeout(hideToast, 3000);
      return () => clearTimeout(timer);
    }
  }, [toast, hideToast]);

  if (!toast) return null;

  const bgColor =
    toast.type === 'error'
      ? 'bg-red-50 border-red-200 text-red-800 dark:bg-red-950 dark:border-red-800 dark:text-red-200'
      : toast.type === 'success'
        ? 'bg-green-50 border-green-200 text-green-800 dark:bg-green-950 dark:border-green-800 dark:text-green-200'
        : 'bg-blue-50 border-blue-200 text-blue-800 dark:bg-blue-950 dark:border-blue-800 dark:text-blue-200';

  return (
    <div className="fixed top-4 right-4 z-50 animate-in fade-in slide-in-from-top-2">
      <div
        role="alert"
        aria-live="assertive"
        className={`flex items-center gap-2 px-4 py-3 rounded-lg border shadow-lg cursor-pointer ${bgColor}`}
        onClick={hideToast}
      >
        <span className="text-sm">{toast.message}</span>
        <button className="ml-2 opacity-60 hover:opacity-100" aria-label="关闭通知">
          ✕
        </button>
      </div>
    </div>
  );
}
