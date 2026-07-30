import { Plus, Trash2, TestTube, Check, X, Loader2 } from 'lucide-react';
import { Toggle, FieldRow, SectionTitle } from './shared';
import type {
  ConfigState,
  ProviderConfigState,
  ProviderModelEntry,
  ModelCapability,
  ModelPreferencesState,
} from '@/lib/types';
import { MODEL_CAPABILITIES } from '@/lib/types';

/* ── Props ── */

interface ModelsTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
  onAddProvider: () => void;
  onRemoveProvider: (index: number) => void;
  onUpdateProvider: (index: number, field: keyof ProviderConfigState, value: unknown) => void;
  onAddModel: (providerIndex: number) => void;
  onRemoveModel: (providerIndex: number, modelIndex: number) => void;
  onUpdateModel: (
    providerIndex: number,
    modelIndex: number,
    field: keyof ProviderModelEntry,
    value: unknown,
  ) => void;
  onToggleModelCapability: (providerIndex: number, modelIndex: number, cap: ModelCapability) => void;
  onUpdatePreference: (key: keyof ModelPreferencesState, provider: string, model: string) => void;
  onTestConnection: (index: number) => void;
  testStatus: Record<number, 'idle' | 'testing' | 'success' | 'error'>;
}

/* ── Capability labels ── */

const CAPABILITY_LABELS: Record<ModelCapability, string> = {
  chat: '对话 (Chat)',
  vision: '视觉 (Vision)',
  'text-embedding': '文本嵌入',
  'multimodal-embedding': '多模态嵌入',
};

/* ── Helpers ── */

type PreferenceKey = keyof ModelPreferencesState;

function getSelectableModels(
  providers: ProviderConfigState[],
  caps: ModelCapability[],
): { provider: string; model: string }[] {
  const result: { provider: string; model: string }[] = [];
  for (const p of providers) {
    if (!p.enabled) continue;
    for (const m of p.models) {
      if (!m.name.trim()) continue;
      if (caps.some((c) => m.capabilities.includes(c))) {
        result.push({ provider: p.name, model: m.name });
      }
    }
  }
  return result;
}

/* ── Component ── */

export default function ModelsTab({
  config,
  onAddProvider,
  onRemoveProvider,
  onUpdateProvider,
  onAddModel,
  onRemoveModel,
  onUpdateModel,
  onToggleModelCapability,
  onUpdatePreference,
  onTestConnection,
  testStatus,
}: ModelsTabProps) {
  return (
    <div>
      <SectionTitle title="模型服务" />
      <p className="text-xs text-[var(--color-text-tertiary)] mb-4">
        管理 AI 模型服务提供商，支持 OpenAI 兼容接口。每个提供商下可配置多个模型及其能力标签。
      </p>

      {/* ── Preferences section ── */}
      <div className="mb-6 p-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
        <h4 className="text-sm font-semibold text-[var(--color-text-primary)] mb-3">默认模型偏好</h4>
        <p className="text-xs text-[var(--color-text-tertiary)] mb-3">
          为每种能力指定首选模型。未设置时将自动匹配第一个符合条件的已启用模型。
        </p>
        <div className="grid grid-cols-3 gap-3">
          {([
            { key: 'chat' as PreferenceKey, label: '对话 (Chat)', caps: ['chat'] as ModelCapability[] },
            {
              key: 'embedding' as PreferenceKey,
              label: '嵌入 (Embedding)',
              caps: ['text-embedding', 'multimodal-embedding'] as ModelCapability[],
            },
            { key: 'vision' as PreferenceKey, label: '视觉 (Vision)', caps: ['vision'] as ModelCapability[] },
          ]).map(({ key, label, caps }) => {
            const current = config.preferences[key];
            const options = getSelectableModels(config.providers, caps);
            return (
              <FieldRow key={key} label={label}>
                <select
                  value={current ? `${current.provider}|${current.model}` : ''}
                  onChange={(e) => {
                    if (!e.target.value) {
                      onUpdatePreference(key, '', '');
                      return;
                    }
                    const [provider, model] = e.target.value.split('|');
                    onUpdatePreference(key, provider, model);
                  }}
                  className="w-full px-2 py-1 text-xs rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                >
                  <option value="">自动选择</option>
                  {options.map((opt) => (
                    <option key={`${opt.provider}|${opt.model}`} value={`${opt.provider}|${opt.model}`}>
                      {opt.provider}/{opt.model}
                    </option>
                  ))}
                </select>
              </FieldRow>
            );
          })}
        </div>
      </div>

      {/* ── Provider cards ── */}
      {config.providers.map((p, pi) => (
        <div key={p.name || pi} className="border border-[var(--color-border)] rounded-lg p-4 mb-3 space-y-3">
          <div className="grid grid-cols-[1fr_1fr_auto_auto] gap-3 items-end">
            <FieldRow label="名称">
              <input
                type="text"
                value={p.name}
                onChange={(e) => onUpdateProvider(pi, 'name', e.target.value)}
                className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                placeholder="openai"
              />
            </FieldRow>
            <FieldRow label="端点 URL">
              <input
                type="text"
                value={p.endpoint}
                onChange={(e) => onUpdateProvider(pi, 'endpoint', e.target.value)}
                className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                placeholder="https://api.openai.com/v1"
              />
            </FieldRow>
            <div className="flex items-center gap-2 pb-1">
              <Toggle checked={p.enabled} onChange={(v) => onUpdateProvider(pi, 'enabled', v)} label="启用" />
            </div>
            <button
              onClick={() => onRemoveProvider(pi)}
              className="p-1.5 rounded-md text-[var(--color-text-tertiary)] hover:text-[var(--color-error)] hover:bg-[var(--color-error-bg)] transition-colors"
              title="删除"
            >
              <Trash2 size={16} />
            </button>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <FieldRow label="API Key">
              <input
                type="password"
                value={p.api_key}
                onChange={(e) => onUpdateProvider(pi, 'api_key', e.target.value)}
                className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                placeholder="sk-... 或 ${ENV_VAR}"
              />
            </FieldRow>
            <div className="flex items-end gap-2">
              <div className="flex-1">
                <FieldRow label="超时 (秒)">
                  <input
                    type="number"
                    min={1}
                    max={3600}
                    value={p.timeout}
                    onChange={(e) => onUpdateProvider(pi, 'timeout', parseInt(e.target.value) || 60)}
                    className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </FieldRow>
              </div>
              <button
                onClick={() => onTestConnection(pi)}
                disabled={testStatus[pi] === 'testing'}
                className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors disabled:opacity-50"
              >
                {testStatus[pi] === 'testing' ? (
                  <Loader2 size={14} className="animate-spin" />
                ) : testStatus[pi] === 'success' ? (
                  <Check size={14} className="text-[var(--color-success)]" />
                ) : testStatus[pi] === 'error' ? (
                  <X size={14} className="text-[var(--color-error)]" />
                ) : (
                  <TestTube size={14} />
                )}
                测试连接
              </button>
            </div>
          </div>

          {/* ── Models under this provider ── */}
          <div className="pt-2 border-t border-[var(--color-border)]">
            <div className="flex items-center justify-between mb-2">
              <span className="text-sm font-medium text-[var(--color-text-primary)]">模型列表</span>
              <button
                onClick={() => onAddModel(pi)}
                className="flex items-center gap-1 text-xs text-accent hover:text-accent-hover transition-colors"
              >
                <Plus size={14} />
                添加模型
              </button>
            </div>
            {p.models.length === 0 && (
              <p className="text-xs text-[var(--color-text-tertiary)] italic">暂无模型，请添加</p>
            )}
            {p.models.map((m, mi) => (
              <div key={m.name || mi} className="flex items-start gap-2 mb-2 p-2 rounded bg-[var(--color-bg-secondary)]">
                <div className="flex-1 min-w-0">
                  <input
                    type="text"
                    value={m.name}
                    onChange={(e) => onUpdateModel(pi, mi, 'name', e.target.value)}
                    className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent mb-1.5"
                    placeholder="模型名称，如 gpt-4"
                  />
                  <div className="flex flex-wrap gap-1">
                    {MODEL_CAPABILITIES.map((cap) => (
                      <label
                        key={cap}
                        className={`inline-flex items-center gap-1 px-1.5 py-0.5 text-xs rounded cursor-pointer border transition-colors ${
                          m.capabilities.includes(cap)
                            ? 'border-accent bg-accent-light text-accent'
                            : 'border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:border-[var(--color-text-tertiary)]'
                        }`}
                      >
                        <input
                          type="checkbox"
                          checked={m.capabilities.includes(cap)}
                          onChange={() => onToggleModelCapability(pi, mi, cap)}
                          className="sr-only"
                        />
                        {CAPABILITY_LABELS[cap]}
                      </label>
                    ))}
                  </div>
                </div>
                <button
                  onClick={() => onRemoveModel(pi, mi)}
                  className="p-1 rounded text-[var(--color-text-tertiary)] hover:text-[var(--color-error)] hover:bg-[var(--color-error-bg)] transition-colors shrink-0"
                  title="删除模型"
                >
                  <Trash2 size={12} />
                </button>
              </div>
            ))}
          </div>
        </div>
      ))}

      <button
        onClick={onAddProvider}
        className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md border border-dashed border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-accent hover:text-accent transition-colors"
      >
        <Plus size={16} />
        添加提供商
      </button>
    </div>
  );
}
