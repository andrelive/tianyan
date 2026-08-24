import { useState, useRef, useEffect } from 'react';
import { useAppStore } from '@/lib/store';
import { useResource } from '@/hooks/use-resource';
import { ChevronDown, Loader2 } from 'lucide-react';
import { getModels, switchModel } from '@/lib/api-client';
import { cn } from '@/lib/utils';
import { toErrorMessage } from '@/lib/errors';

/** ghost：一体式输入卡片内的无边框变体（外框由父组件统一提供）。 */
export default function ModelSelector({ ghost = false }: { ghost?: boolean }) {
  const selectedModel = useAppStore((s) => s.selectedModel);
  const setModel = useAppStore((s) => s.setModel);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // 模型列表加载（useResource：加载/竞态收敛；副作用留在 fetcher 内：
  // 过滤 chat 能力 + 写入 store + 首次自动选中；失败静默——按钮禁用态解除即可）
  const setChatModelsStore = useAppStore((s) => s.setChatModels);
  const { loading } = useResource(
    async () => {
      const data = await getModels();
      // Filter models with "chat" capability；保留完整信息（含每模型思考档位）
      const chat = data.models.filter((m) => m.capabilities.includes('chat'));
      setChatModelsStore(chat);
      // Auto-select first model if none selected
      if (chat.length > 0 && !selectedModel) {
        const preferred = data.preferences.chat;
        if (preferred) {
          setModel(preferred.model);
        } else {
          setModel(chat[0].name);
        }
      }
      return data;
    },
    [],
    { errorFallback: '加载模型列表失败' },
  );

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const escapeHandler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', handler);
    document.addEventListener('keydown', escapeHandler);
    return () => {
      document.removeEventListener('mousedown', handler);
      document.removeEventListener('keydown', escapeHandler);
    };
  }, []);

  const displayModels = useAppStore((s) => s.chatModels);
  const currentModel = selectedModel || displayModels[0]?.name || '选择模型';

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen(!open)}
        disabled={loading}
        role="combobox"
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label="选择模型"
        className={cn(
          'flex items-center gap-2 px-2.5 py-1.5 text-sm rounded-lg transition-colors disabled:opacity-50',
          ghost
            ? 'text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
            : 'border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]',
        )}
      >
        {loading ? <Loader2 size={14} className="animate-spin" /> : <span>{currentModel}</span>}
        <ChevronDown className="w-4 h-4 text-[var(--color-text-tertiary)]" />
      </button>

      {open && displayModels.length > 0 && (
        <div
          role="listbox"
          aria-label="模型列表"
          className="absolute right-0 bottom-full mb-1 w-56 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1"
        >
          {displayModels.map((model) => (
            <button
              key={model.name}
              role="option"
              aria-selected={model.name === currentModel}
              onClick={() => {
                setModel(model.name);
                setOpen(false);
                // 本地状态先行（UI 不依赖网络），后端持久化失败不阻塞交互
                switchModel(model.name, 'chat').catch((err: unknown) => {
                  const message = toErrorMessage(err, '未知错误');
                  useAppStore.getState().showToast('切换模型失败: ' + message, 'error');
                });
              }}
              className={
                'w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors ' +
                (model.name === currentModel
                  ? 'text-blue-600 dark:text-blue-400 font-medium'
                  : 'text-[var(--color-text-primary)]')
              }
            >
              {model.name}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
