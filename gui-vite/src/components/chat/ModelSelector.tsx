import { useState, useRef, useEffect } from 'react';
import { useAppStore } from '@/lib/store';
import { ChevronDown, Loader2 } from 'lucide-react';
import { apiGet } from '@/lib/api-client';
import type { ModelsResponse, ModelRef } from '@/lib/types';

export default function ModelSelector() {
  const selectedModel = useAppStore((s) => s.selectedModel);
  const setModel = useAppStore((s) => s.setModel);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // Fetch models from backend
  const [chatModels, setChatModels] = useState<ModelRef[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    apiGet<ModelsResponse>('/config/models')
      .then((data) => {
        if (cancelled) return;
        // Filter models with "chat" capability
        const chat = data.models
          .filter((m) => m.capabilities.includes('chat'))
          .map((m) => ({ provider: m.provider, model: m.name }));
        setChatModels(chat);
        setLoading(false);

        // Auto-select first model if none selected
        if (chat.length > 0 && !selectedModel) {
          const preferred = data.preferences.chat;
          if (preferred) {
            setModel(preferred.model);
          } else {
            setModel(chat[0].model);
          }
        }
      })
      .catch(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps -- run once on mount
  }, []);

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

  const currentModel = selectedModel || chatModels[0]?.model || '选择模型';
  const displayModels = chatModels.length > 0
    ? chatModels.map((m) => m.model)
    : [];

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen(!open)}
        disabled={loading}
        role="combobox"
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label="选择模型"
        className="flex items-center gap-2 px-3 py-1.5 text-sm rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-primary)] hover:bg-[var(--color-bg-hover)] text-[var(--color-text-primary)] transition-colors disabled:opacity-50"
      >
        {loading ? (
          <Loader2 size={14} className="animate-spin" />
        ) : (
          <span>{currentModel}</span>
        )}
        <ChevronDown className="w-4 h-4 text-[var(--color-text-tertiary)]" />
      </button>

      {open && displayModels.length > 0 && (
        <div role="listbox" aria-label="模型列表" className="absolute right-0 top-full mt-1 w-56 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1">
          {displayModels.map((model) => (
            <button
              key={model}
              role="option"
              aria-selected={model === currentModel}
              onClick={() => {
                setModel(model);
                setOpen(false);
              }}
              className={`w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors ${
                model === currentModel
                  ? 'text-blue-600 dark:text-blue-400 font-medium'
                  : 'text-[var(--color-text-primary)]'
              }`}
            >
              {model}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
