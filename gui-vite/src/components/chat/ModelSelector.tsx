import { useState, useRef, useEffect } from 'react';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
import { useResource } from '@/hooks/use-resource';
import { ChevronDown, Loader2 } from 'lucide-react';
import { getModels, setSessionModel } from '@/lib/api-client';
import { cn } from '@/lib/utils';
import { toErrorMessage } from '@/lib/errors';
import type { SelectedModel } from '@/lib/store';

/** ghost：一体式输入卡片内的无边框变体（外框由父组件统一提供）。 */
export default function ModelSelector({ ghost = false }: { ghost?: boolean }) {
  const selectedModel = useAppStore((s) => s.selectedModel);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // 模型列表加载（useResource：加载/竞态收敛；副作用留在 fetcher 内：
  // 过滤 chat 能力 + 写入 store + 记录全局默认（视图回落统一走 syncModelView）；
  // 失败静默——按钮禁用态解除即可）
  const setChatModelsStore = useAppStore((s) => s.setChatModels);
  const { loading } = useResource(
    async () => {
      const data = await getModels();
      // Filter models with "chat" capability；保留完整信息（含每模型思考档位）
      const chat = data.models.filter((m) => m.capabilities.includes('chat'));
      setChatModelsStore(chat);
      // 全局默认（preferences.chat；缺省回落列表首项）——新会话 / 无记录会话
      // 的视图回落来源；当前会话若已有选择则由 syncModelView 优先使用之。
      const preferred = data.preferences.chat;
      const fallback: SelectedModel | null = preferred
        ? { provider: preferred.provider, model: preferred.model }
        : chat[0]
          ? { provider: chat[0].provider, model: chat[0].name }
          : null;
      const st = useAppStore.getState();
      st.setDefaultModelSelection(fallback);
      st.syncModelView(st.currentSessionId ?? PENDING_SESSION_KEY);
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
  /** 当前选中（未选中时回退列表首项展示——与后端默认解析一致）。
      provider+model 复合匹配：同名模型跨 provider 时不误标、不误切。 */
  const current: SelectedModel | null =
    selectedModel ??
    (displayModels[0]
      ? { provider: displayModels[0].provider, model: displayModels[0].name }
      : null);
  const isCurrent = (m: { provider: string; name: string }) =>
    current !== null && current.provider === m.provider && current.model === m.name;

  /** 选择模型：写**当前会话**的模型选择（会话本地；不改全局默认）。
      - 已创建会话 → 同步后端（PUT /sessions/{id}/model，写会话头部）；
      - 未创建会话 → 存 PENDING 槽，首条消息创建会话后固化（见 ChatPanel）。 */
  const selectModel = (m: {
    provider: string;
    name: string;
    reasoning_efforts?: string[] | null;
  }) => {
    const st = useAppStore.getState();
    const key = st.currentSessionId ?? PENDING_SESSION_KEY;
    // 思考档位跟随：新模型档位集不含当前档位 → 回落 off（不发 thinking）
    const declared = m.reasoning_efforts ?? [];
    const keepThinking = st.thinkingEffort !== 'off' && declared.includes(st.thinkingEffort);
    const selection = {
      provider: m.provider,
      model: m.name,
      ...(keepThinking ? { thinking: st.thinkingEffort } : {}),
    };
    st.setSessionSelection(key, selection);
    st.syncModelView(key);
    if (st.currentSessionId) {
      setSessionModel(st.currentSessionId, selection).catch((err: unknown) => {
        const message = toErrorMessage(err, '未知错误');
        useAppStore.getState().showToast('设置会话模型失败: ' + message, 'error');
      });
    }
  };

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen(!open)}
        disabled={loading}
        role="combobox"
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label="选择模型"
        title={current ? `${current.provider} / ${current.model}` : undefined}
        className={cn(
          'flex items-center gap-2 px-2.5 py-1.5 text-sm rounded-lg transition-colors disabled:opacity-50',
          ghost
            ? 'text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
            : 'border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]',
        )}
      >
        {loading ? (
          <Loader2 size={14} className="animate-spin" />
        ) : (
          <>
            <span className="max-w-[12rem] truncate">{current?.model ?? '选择模型'}</span>
            {current && (
              <span className="text-xs text-[var(--color-text-tertiary)] shrink-0">
                {current.provider}
              </span>
            )}
          </>
        )}
        <ChevronDown className="w-4 h-4 text-[var(--color-text-tertiary)]" />
      </button>

      {open && displayModels.length > 0 && (
        <div
          role="listbox"
          aria-label="模型列表"
          className="absolute right-0 bottom-full mb-1 w-64 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1"
        >
          {displayModels.map((model) => (
            <button
              key={`${model.provider}/${model.name}`}
              role="option"
              aria-selected={isCurrent(model)}
              onClick={() => {
                selectModel(model);
                setOpen(false);
              }}
              className={cn(
                'w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors flex items-center justify-between gap-3',
                isCurrent(model)
                  ? 'text-blue-600 dark:text-blue-400 font-medium'
                  : 'text-[var(--color-text-primary)]',
              )}
            >
              <span className="truncate">{model.name}</span>
              <span className="text-xs text-[var(--color-text-tertiary)] shrink-0">
                {model.provider}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
