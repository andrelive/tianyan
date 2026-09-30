import { useState, useRef, useEffect } from 'react';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
import { ChevronDown, Check } from 'lucide-react';
import { setSessionModel } from '@/lib/api-client';
import { cn } from '@/lib/utils';
import { toErrorMessage } from '@/lib/errors';

/**
 * 内置「default」档位（**不附加思考参数，是否思考由模型自行决定**——思考模型
 * 缺省仍会输出推理；不属于模型声明，恒提供）。值保持 `'off'`（会话选择 /
 * 请求语义不变，仅展示名如实化）。
 */
const OFF_EFFORT = 'off';
/** 档位展示名：`off` 值如实呈现为「default」（不附加参数，由模型决定）。 */
function effortLabel(v: string): string {
  return v === OFF_EFFORT ? 'default' : v;
}

/**
 * 会话级思考强度选择：档位集来自**当前模型自己声明的值**（后端 /config/models
 * 返回 reasoning_efforts，显式配置 > 内置模型表），**原样展示不做本地翻译**
 * （厂商档位可能为 low/high/max 等任意值）；模型不支持思考时隐藏。
 * 内置「default」档（值 `off`）**如实展示为「default」**——不附加思考参数，
 * 是否思考由模型自行决定（思考模型缺省仍会输出推理）。
 *
 * 选择写入**当前会话**（与模型选择同一个会话级对象；对齐 DSH 的
 * Session-local model selection）：已创建会话同步后端（会话头部），
 * 未创建会话存 PENDING 槽（首条消息创建会话后固化）。
 */
/** ghost：一体式输入卡片内的无边框变体。 */
export default function ThinkingSelect({ ghost = false }: { ghost?: boolean }) {
  const thinkingEffort = useAppStore((s) => s.thinkingEffort);
  const setThinkingEffort = useAppStore((s) => s.setThinkingEffort);
  const selectedModel = useAppStore((s) => s.selectedModel);
  const chatModels = useAppStore((s) => s.chatModels);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // 当前模型声明的档位值（每个模型自己的；缺省 = 不支持思考 → 隐藏）
  // provider+model 复合匹配：同名模型跨 provider 时取真正选中项的声明
  const declared: string[] | undefined =
    (selectedModel
      ? chatModels.find(
          (m) => m.provider === selectedModel.provider && m.name === selectedModel.model,
        )
      : undefined
    )?.reasoning_efforts ?? undefined;
  const supportsThinking =
    !!declared && declared.length > 0 && !(declared.length === 1 && declared[0] === OFF_EFFORT);

  // 选项：恒含「default」，外加模型声明的档位值（去重、原样显示）
  const options: string[] = supportsThinking
    ? [OFF_EFFORT, ...declared!.filter((v) => v !== OFF_EFFORT)]
    : [];

  // 模型切换后若已选档位不在新模型档位集内 → 重置为default（视图兜底；
  // 会话记录在下次切会话 syncModelView 时自愈）
  useEffect(() => {
    if (supportsThinking && !options.includes(thinkingEffort)) {
      setThinkingEffort(OFF_EFFORT);
    }
  }, [declared, thinkingEffort, setThinkingEffort]); // eslint-disable-line react-hooks/exhaustive-deps -- options 由 declared 派生

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
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

  if (!supportsThinking) return null;

  const current = thinkingEffort;

  /** 选择档位：写当前会话的模型选择（thinking 缺省 = off）。 */
  const selectEffort = (opt: string) => {
    const st = useAppStore.getState();
    const base = st.selectedModel;
    if (!base) return;
    const key = st.currentSessionId ?? PENDING_SESSION_KEY;
    const selection = {
      provider: base.provider,
      model: base.model,
      ...(opt === OFF_EFFORT ? {} : { thinking: opt }),
    };
    st.setSessionSelection(key, selection);
    st.syncModelView(key);
    if (st.currentSessionId) {
      setSessionModel(st.currentSessionId, selection).catch((err: unknown) => {
        const message = toErrorMessage(err, '未知错误');
        useAppStore.getState().showToast('设置思考强度失败: ' + message, 'error');
      });
    }
  };

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-label="思考强度"
        aria-expanded={open}
        aria-haspopup="listbox"
        title={
          current === OFF_EFFORT
            ? '思考强度：default（不附加思考参数，是否思考由模型决定）'
            : '思考强度：' + current + '（当前模型声明的档位）'
        }
        className={cn(
          'flex items-center gap-1.5 px-2.5 py-1.5 text-sm rounded-lg transition-colors',
          ghost
            ? 'text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-secondary)]'
            : 'border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]',
        )}
      >
        <span>{'思考 ' + effortLabel(current)}</span>
        <ChevronDown className={cn('w-3.5 h-3.5 transition-transform', open && 'rotate-180')} />
      </button>

      {open && (
        <div
          role="listbox"
          aria-label="思考强度列表"
          className="absolute right-0 bottom-full mb-1 w-48 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-lg z-50 py-1"
        >
          {options.map((opt) => (
            <button
              key={opt}
              role="option"
              aria-selected={opt === current}
              onClick={() => {
                selectEffort(opt);
                setOpen(false);
              }}
              className={cn(
                'w-full text-left px-3 py-2 text-sm hover:bg-[var(--color-bg-hover)] transition-colors flex items-center justify-between gap-2',
                opt === current
                  ? 'text-blue-600 dark:text-blue-400 font-medium'
                  : 'text-[var(--color-text-primary)]',
              )}
            >
              <span>{effortLabel(opt)}</span>
              {opt === current && <Check size={14} className="shrink-0" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
