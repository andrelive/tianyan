import { useEffect, useState } from 'react';
import { ChevronDown, Loader2, Plus } from 'lucide-react';
import { SectionTitle, FieldRow } from './shared';
import type {
  ConfigState,
  ProviderConfigState,
  ProviderModelEntry,
  ModelCapability,
  ModelPreferencesState,
  DiscoveredModelInfo,
  ProviderPresetInfo,
  ProviderProtocol,
} from '@/lib/types';
import { useProviderScan } from '@/hooks/use-provider-scan';
import { fetchProviderPresets } from '@/lib/api-client';
import { toErrorMessage } from '@/lib/errors';
import ProviderCard from './ProviderCard';

/* ── Props ── */

interface ModelsTabProps {
  config: ConfigState;
  /** 添加提供商；可传入预置草稿（预置选择器一键填充，ADR-046）。 */
  onAddProvider: (draft?: Partial<ProviderConfigState>) => void;
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

  /* 预置选择器（ADR-046）：懒加载预置列表 → 一键填草稿（name + endpoint） */
  const [presetsOpen, setPresetsOpen] = useState(false);
  const [presets, setPresets] = useState<ProviderPresetInfo[] | null>(null);
  const [presetsError, setPresetsError] = useState<string | null>(null);

  useEffect(() => {
    if (!presetsOpen || presets !== null || presetsError) return;
    let cancelled = false;
    void (async () => {
      try {
        const resp = await fetchProviderPresets();
        if (!cancelled) setPresets(resp.presets);
      } catch (err: unknown) {
        if (!cancelled) setPresetsError(toErrorMessage(err, '预置列表加载失败'));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [presetsOpen, presets, presetsError]);

  const pickPreset = (preset?: ProviderPresetInfo) => {
    onAddProvider(preset ? { name: preset.id, endpoint: preset.endpoint } : undefined);
    setPresetsOpen(false);
  };

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
          resolvedEndpoints={config.resolvedEndpoints}
          providerPresets={config.providerPresets}
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

      <div className="relative inline-block">
        <button
          onClick={() => setPresetsOpen((open) => !open)}
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md border border-dashed border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-accent hover:text-accent transition-colors"
          aria-haspopup="menu"
          aria-expanded={presetsOpen}
        >
          <Plus size={16} />
          添加提供商
          <ChevronDown size={14} />
        </button>
        {presetsOpen && (
          <>
            {/* 点击外部关闭（透明背板） */}
            <div className="fixed inset-0 z-10" onClick={() => setPresetsOpen(false)} />
            <div
              role="menu"
              className="absolute left-0 top-full mt-1 z-20 w-80 max-h-80 overflow-y-auto rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] shadow-lg py-1"
            >
              <button
                role="menuitem"
                onClick={() => pickPreset()}
                className="w-full text-left px-3 py-2 text-sm text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]"
              >
                空白提供商
              </button>
              {presets === null && !presetsError && (
                <div className="flex items-center gap-2 px-3 py-2 text-xs text-[var(--color-text-tertiary)]">
                  <Loader2 size={12} className="animate-spin" />
                  加载预置…
                </div>
              )}
              {presetsError && (
                <div className="px-3 py-2 text-xs text-red-400" role="alert">
                  {presetsError}
                  <button
                    className="ml-2 underline hover:text-red-300"
                    onClick={() => setPresetsError(null)}
                  >
                    重试
                  </button>
                </div>
              )}
              {(presets ?? []).map((preset) => (
                <button
                  key={preset.id}
                  role="menuitem"
                  onClick={() => pickPreset(preset)}
                  className="w-full text-left px-3 py-2 hover:bg-[var(--color-bg-hover)]"
                >
                  <div className="text-sm text-[var(--color-text-primary)]">
                    {preset.display_name}
                  </div>
                  <div className="text-xs text-[var(--color-text-tertiary)]">
                    {preset.description}
                  </div>
                  {preset.dynamic_headers.length > 0 && (
                    <div className="text-[10px] text-[var(--color-text-tertiary)] mt-0.5">
                      自动注入会话头：{preset.dynamic_headers.map((h) => h.name).join('、')}
                    </div>
                  )}
                </button>
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
