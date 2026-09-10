import { useState } from 'react';
import { Plus } from 'lucide-react';
import { SectionTitle, FieldRow } from './shared';
import type {
  ConfigState,
  ProviderConfigState,
  ProviderModelEntry,
  ModelCapability,
  ModelPreferencesState,
  DiscoveredModelInfo,
  ProviderProtocol,
} from '@/lib/types';
import { useProviderScan } from '@/hooks/use-provider-scan';
import ProviderCard from './ProviderCard';

/* ── Props ── */

interface ModelsTabProps {
  config: ConfigState;
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
  onToggleModelCapability: (
    providerIndex: number,
    modelIndex: number,
    cap: ModelCapability,
  ) => void;
  onUpdatePreference: (key: keyof ModelPreferencesState, provider: string, model: string) => void;
  onTestConnection: (index: number) => void;
  testStatus: Record<number, 'idle' | 'testing' | 'success' | 'error'>;
  onAddScannedModels: (providerIndex: number, models: DiscoveredModelInfo[]) => void;
}

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

/* ── Component（编排：偏好区 + Provider 卡片列表；扫描状态机在 useProviderScan） ── */

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
  onAddScannedModels,
}: ModelsTabProps) {
  /* 每个 provider 的模型列表折叠态（默认收起；仅 UI state，不持久化） */
  const [collapsed, setCollapsed] = useState<Record<number, boolean>>({});

  /* 扫描状态机（useProviderScan：五元组 per-provider 收敛 + 可单测） */
  const { scanState, setScanProtocol, scan, togglePicked, adoptPicked } = useProviderScan(
    (pi) => new Set((config.providers[pi]?.models ?? []).map((m) => m.name.trim())),
  );

  const handleAdopt = (pi: number) => {
    const models = adoptPicked(pi);
    if (models.length === 0) return;
    onAddScannedModels(pi, models);
  };

  return (
    <div>
      <SectionTitle title="模型服务" />
      <p className="text-xs text-[var(--color-text-tertiary)] mb-4">
        管理 AI 模型服务提供商，支持 OpenAI 兼容接口。每个提供商下可配置多个模型及其能力标签。
      </p>

      {/* ── Preferences section ── */}
      <div className="mb-6 p-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
        <h4 className="text-sm font-semibold text-[var(--color-text-primary)] mb-3">
          默认模型偏好
        </h4>
        <p className="text-xs text-[var(--color-text-tertiary)] mb-3">
          为每种能力指定首选模型。未设置时将自动匹配第一个符合条件的已启用模型。
        </p>
        <div className="grid grid-cols-3 gap-3">
          {[
            {
              key: 'chat' as PreferenceKey,
              label: '对话 (Chat)',
              caps: ['chat'] as ModelCapability[],
            },
            {
              key: 'embedding' as PreferenceKey,
              label: '嵌入 (Embedding)',
              caps: ['text-embedding', 'multimodal-embedding'] as ModelCapability[],
            },
            {
              key: 'vision' as PreferenceKey,
              label: '视觉 (Vision)',
              caps: ['vision'] as ModelCapability[],
            },
          ].map(({ key, label, caps }) => {
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
                    <option
                      key={`${opt.provider}|${opt.model}`}
                      value={`${opt.provider}|${opt.model}`}
                    >
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
        <ProviderCard
          key={pi}
          provider={p}
          collapsed={collapsed[pi] ?? true}
          onToggleCollapsed={() => setCollapsed((prev) => ({ ...prev, [pi]: !(prev[pi] ?? true) }))}
          testStatus={testStatus[pi] ?? 'idle'}
          scanState={
            scanState[pi] ?? {
              protocol: 'openai' as ProviderProtocol,
              scanning: false,
              models: [],
              picked: [],
              error: null,
            }
          }
          resolvedSpecs={config.resolvedSpecs}
          modelCatalog={config.modelCatalog}
          onUpdateProvider={(field, value) => onUpdateProvider(pi, field, value)}
          onRemoveProvider={() => onRemoveProvider(pi)}
          onTestConnection={() => onTestConnection(pi)}
          onScan={(endpoint, protocol, name) => void scan(pi, endpoint, protocol, name)}
          onSetScanProtocol={(protocol) => setScanProtocol(pi, protocol)}
          onTogglePicked={(name) => togglePicked(pi, name)}
          onAdoptPicked={() => handleAdopt(pi)}
          onAddModel={() => onAddModel(pi)}
          onRemoveModel={(mi) => onRemoveModel(pi, mi)}
          onUpdateModel={(mi, field, value) => onUpdateModel(pi, mi, field, value)}
          onToggleModelCapability={(mi, cap) => onToggleModelCapability(pi, mi, cap)}
        />
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
