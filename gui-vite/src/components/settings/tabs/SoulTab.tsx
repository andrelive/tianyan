import { useState, useEffect, useCallback } from 'react';
import { Save, RotateCcw, Loader2, AlertCircle } from 'lucide-react';
import { fetchSoulContent, updateSoulContent, fetchDefaultSoul } from '@/lib/api-client';
import { useAppStore } from '@/lib/store';

export default function SoulTab() {
  const showToast = useAppStore((s) => s.showToast);

  const [content, setContent] = useState('');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    fetchSoulContent()
      .then((res) => {
        if (!cancelled) {
          setContent(res.content);
          setLoading(false);
        }
      })
      .catch((err: Error) => {
        if (!cancelled) {
          setError(err.message || '加载失败');
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleSave = useCallback(async () => {
    setSaving(true);
    setError(null);
    try {
      await updateSoulContent(content);
      showToast('智能体人格已保存，下次对话生效', 'success');
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '保存失败';
      setError(msg);
    } finally {
      setSaving(false);
    }
  }, [content, showToast]);

  const handleRestore = useCallback(async () => {
    setRestoring(true);
    setError(null);
    try {
      const res = await fetchDefaultSoul();
      setContent(res.content);
      showToast('已恢复默认人格（尚未保存，请点击保存）', 'info');
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '加载默认人格失败';
      setError(msg);
    } finally {
      setRestoring(false);
    }
  }, [showToast]);

  if (loading) {
    return (
      <div
        className="flex items-center justify-center py-20"
        aria-live="polite"
        aria-label="加载中"
      >
        <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
      </div>
    );
  }

  const lineCount = content.split('\n').length;
  const charCount = content.length;

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-base font-semibold text-[var(--color-text-primary)]">智能体人格</h3>
        <div className="text-xs text-[var(--color-text-tertiary)]">
          {lineCount} 行 · {charCount.toLocaleString()} 字符
        </div>
      </div>

      <p className="text-sm text-[var(--color-text-secondary)] mb-4">
        编辑智能体的核心人格提示词。修改后将在下次对话中生效。
      </p>

      <textarea
        value={content}
        onChange={(e) => {
          setContent(e.target.value);
          setError(null);
        }}
        className="w-full min-h-[400px] px-4 py-3 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-sm text-[var(--color-text-primary)] font-mono leading-relaxed resize-y focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500"
        placeholder="输入智能体的核心人格..."
        aria-label="智能体人格内容"
      />

      {error && (
        <div
          role="alert"
          className="mt-3 flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
        >
          <AlertCircle size={16} />
          <span>{error}</span>
        </div>
      )}

      <div className="mt-4 flex items-center gap-3">
        <button
          onClick={handleSave}
          disabled={saving || !content.trim()}
          className="flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-md bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 transition-colors"
        >
          {saving ? <Loader2 size={16} className="animate-spin" /> : <Save size={16} />}
          {saving ? '保存中...' : '保存人格'}
        </button>
        <button
          onClick={handleRestore}
          disabled={restoring}
          className="flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 transition-colors"
        >
          {restoring ? <Loader2 size={16} className="animate-spin" /> : <RotateCcw size={16} />}
          {restoring ? '加载中...' : '恢复默认'}
        </button>
      </div>
    </div>
  );
}
