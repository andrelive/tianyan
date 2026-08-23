import { useCallback, useRef, useState } from 'react';
import { usePolling } from '@/hooks/use-polling';
import { toErrorMessage } from '@/lib/errors';
import { ClipboardList, X } from 'lucide-react';
import { fetchClipboardPending, respondClipboard } from '@/lib/api-client';
import { useAppStore } from '@/lib/store';

/**
 * 剪贴板确认条（T1 路线：复制即记忆）。
 *
 * 剪贴板监听由桌面壳（Tauri）完成——捕获内容 POST 到 server 后：
 * - `auto_capture` 开启：直接沉淀，无确认条
 * - 否则：server 保存 pending，本组件轮询展示确认条
 *   [存入记忆] [存入知识库] [忽略]
 */
export default function ClipboardConfirmBar() {
  const showToast = useAppStore((s) => s.showToast);
  const [pending, setPending] = useState<{ id: string; text: string } | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const dismissedRef = useRef<string | null>(null);

  // 有 pending 时快轮询（2s：确认/忽略后停止）；无 pending 时低频探测
  // （4s：捕获到达后立即显示）。两组轮询互斥启用，消除原实现的重复请求。
  const pollFast = useCallback(async () => {
    if (!pending) return;
    try {
      const next = await fetchClipboardPending();
      if (!next || next.id === dismissedRef.current) {
        if (!next) setPending(null);
      } else if (next.id !== pending.id) {
        setPending({ id: next.id, text: next.text });
      }
    } catch {
      // 轮询失败静默（下次重试）
    }
  }, [pending]);
  const pollSlow = useCallback(async () => {
    try {
      const next = await fetchClipboardPending();
      if (next && next.id !== dismissedRef.current) {
        setPending({ id: next.id, text: next.text });
      }
    } catch {
      // 静默
    }
  }, []);
  usePolling(pollFast, 2000, { enabled: !!pending });
  usePolling(pollSlow, 4000, { enabled: !pending });

  const handleAction = async (action: 'remember' | 'knowledge' | 'ignore') => {
    if (submitting) return;
    setSubmitting(true);
    try {
      const resp = await respondClipboard(action, action === 'ignore' ? undefined : pending?.text);
      if (action !== 'ignore') {
        showToast(action === 'remember' ? '已存入记忆' : '已导入知识库', 'success');
      }
      dismissedRef.current = pending?.id ?? null;
      setPending(null);
      void resp;
    } catch (err: unknown) {
      showToast(`剪贴板操作失败: ${toErrorMessage(err, '未知错误')}`, 'error');
    } finally {
      setSubmitting(false);
    }
  };

  if (!pending) return null;

  return (
    <div className="fixed bottom-24 left-1/2 -translate-x-1/2 z-50 w-[min(560px,92vw)]">
      <div className="flex items-start gap-3 p-3 rounded-xl border border-[var(--color-border)] bg-[var(--color-bg-secondary)] shadow-lg">
        <ClipboardList className="w-4 h-4 mt-0.5 shrink-0 text-[var(--color-text-tertiary)]" />
        <div className="flex-1 min-w-0">
          <p className="text-xs font-medium text-[var(--color-text-primary)] mb-1">
            检测到复制内容 — 是否沉淀？
          </p>
          <p className="text-xs text-[var(--color-text-secondary)] line-clamp-2 break-all whitespace-pre-wrap">
            {pending.text}
          </p>
        </div>
        <div className="flex items-center gap-1.5 shrink-0">
          <button
            onClick={() => void handleAction('remember')}
            disabled={submitting}
            className="px-2 py-1 text-xs rounded bg-[var(--color-accent)] text-white hover:opacity-90 disabled:opacity-50 transition-opacity"
          >
            存入记忆
          </button>
          <button
            onClick={() => void handleAction('knowledge')}
            disabled={submitting}
            className="px-2 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 transition-colors"
          >
            导入知识库
          </button>
          <button
            onClick={() => void handleAction('ignore')}
            disabled={submitting}
            aria-label="忽略"
            title="忽略"
            className="p-1 rounded text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-hover)] transition-colors"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>
    </div>
  );
}
