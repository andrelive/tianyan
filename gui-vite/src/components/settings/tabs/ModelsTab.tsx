import { useState, useEffect, useRef } from 'react';
import {
  Plus,
  Trash2,
  TestTube,
  Check,
  X,
  Loader2,
  RefreshCw,
  ChevronDown,
  ChevronRight,
} from 'lucide-react';
import { Toggle, FieldRow, SectionTitle } from './shared';
import type {
  ConfigState,
  ProviderConfigState,
  ProviderModelEntry,
  ModelCapability,
  ModelCatalogInfo,
  ModelPreferencesState,
  DiscoveredModelInfo,
  ProviderProtocol,
  ResolvedModelSpec,
} from '@/lib/types';
import { MODEL_CAPABILITIES } from '@/lib/types';
import { scanProviderModels } from '@/lib/api-client';

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

/* ── Capability labels ── */

const CAPABILITY_LABELS: Record<ModelCapability, string> = {
  chat: '对话 (Chat)',
  vision: '视觉 (Vision)',
  'text-embedding': '文本嵌入',
  'multimodal-embedding': '多模态嵌入',
};



/**
 * 模型的思考强度档位值编辑器（每个模型自己声明的档位集，逗号分隔）：
 * 值完全自由（如 low/high/max，由厂商/用户定义），不做任何本地映射或过滤；
 * 留空 = 不支持思考。对话中选择强度时原样展示这些值。
 *
 * 实时提交（随输入同步 config，而非失焦提交）：失焦提交会与"保存"按钮
 * 的闭包 config 产生竞态——blur 的 setState 未重渲染时保存读到旧值，
 * 导致刚配置的档位保存后被覆盖为空。localJoin 标记本地产生的值，
 * 避免受控回写把用户输入原文（如尾逗号）重置掉。
 */
function EffortInput({
  value,
  onChange,
}: {
  value: string;
  onChange: (efforts: string[] | undefined) => void;
}) {
  const [draft, setDraft] = useState(value);
  // 本地输入产生的规范化值：外部变化（重载）与之不同才回填草稿
  const localJoin = useRef(value);

  useEffect(() => {
    if (value !== localJoin.current) {
      setDraft(value);
      localJoin.current = value;
    }
  }, [value]);

  const handleChange = (raw: string) => {
    setDraft(raw);
    const tokens = raw.split(/[,，]/).map((v) => v.trim()).filter(Boolean);
    const joined = (tokens.length > 0 ? tokens : []).join(', ');
    localJoin.current = joined;
    onChange(tokens.length > 0 ? tokens : undefined);
  };

  return (
    <input
      type="text"
      aria-label="思考强度档位"
      value={draft}
      onChange={(e) => handleChange(e.target.value)}
      placeholder="留空 = 不支持思考（如 low, high, max）"
      className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
    />
  );
}

/* ── Helpers ── */

type PreferenceKey = keyof ModelPreferencesState;

/** 数字紧凑缩写：>=1M → "1M"；>=1K → "128K"；否则原样。 */
function formatCompactNumber(n: number): string {
  if (n >= 1_000_000) return `${Math.floor(n / 1_000_000)}M`;
  if (n >= 1_000) return `${Math.floor(n / 1_000)}K`;
  return String(n);
}

/** 发现模型的规格摘要（上下文/输出存在时展示）。 */
function formatDiscoveredSpecs(m: DiscoveredModelInfo): string {
  const parts: string[] = [];
  if (m.context_length) parts.push(`${formatCompactNumber(m.context_length)} 上下文`);
  if (m.max_output_tokens) parts.push(`${formatCompactNumber(m.max_output_tokens)} 输出`);
  return parts.join(' / ');
}

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

/**
 * 生效规格只读行（后端解析结果）。key = "{provider}/{model}"；
 * 旧后端无 model_specs 数据时不渲染。展示字段与规格输入框门控一致。
 */
function ResolvedSpecRow({
  spec,
  model,
  catalog,
}: {
  spec: ResolvedModelSpec;
  model: ProviderModelEntry;
  catalog?: ModelCatalogInfo;
}) {
  const isChatOrVision = model.capabilities.some(
    (cap) => cap === 'chat' || cap === 'vision',
  );
  const isEmbedding = model.capabilities.some(
    (cap) => cap === 'text-embedding' || cap === 'multimodal-embedding',
  );
  const parts: string[] = [];
  if (isChatOrVision) {
    parts.push(`${formatCompactNumber(spec.context_length)} 上下文`);
    parts.push(`${formatCompactNumber(spec.max_output_tokens)} 输出`);
  }
  if (isEmbedding) {
    parts.push(`嵌入上限 ${formatCompactNumber(spec.max_input_tokens)}`);
  }
  if (parts.length === 0) return null;

  const hasExplicit = model.context_length ?? model.max_output_tokens ?? model.max_input_tokens;
  return (
    <div className="flex items-center gap-2 mt-1.5 flex-wrap">
      <span className="text-xs text-[var(--color-text-tertiary)]">
        生效规格：{parts.join(' / ')}
      </span>
      {catalog && (
        <span className="text-xs text-[var(--color-text-tertiary)]">
          · {catalog.display_name ?? '内置目录'}
          {catalog.reasoning_efforts && catalog.reasoning_efforts.length > 0
            ? `（默认档位：${catalog.reasoning_efforts.join(', ')}）`
            : ''}
        </span>
      )}
      {hasExplicit ? (
        <span className="px-1.5 py-0.5 text-[10px] rounded-md bg-accent/10 text-accent border border-accent/20">
          自定义
        </span>
      ) : (
        <span className="px-1.5 py-0.5 text-[10px] rounded-md bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)] border border-[var(--color-border)]">
          自动匹配
        </span>
      )}
    </div>
  );
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
  onAddScannedModels,
}: ModelsTabProps) {
  /* Scan state per provider (runtime only, not persisted) */
  const [scanState, setScanState] = useState<
    Record<
      number,
      {
        protocol: ProviderProtocol;
        scanning: boolean;
        models: DiscoveredModelInfo[];
        picked: string[];
        error: string | null;
      }
    >
  >({});
  /* 每个 provider 的模型列表折叠态（默认收起；仅 UI state，不持久化） */
  const [collapsed, setCollapsed] = useState<Record<number, boolean>>({});

  const handleScanModels = async (
    pi: number,
    endpoint: string,
    protocol: ProviderProtocol,
    providerName: string,
  ) => {
    setScanState((prev) => ({
      ...prev,
      [pi]: {
        ...(prev[pi] ?? { protocol, models: [], error: null, picked: [] }),
        scanning: true,
        error: null,
      },
    }));
    try {
      const resp = await scanProviderModels(endpoint, protocol, providerName);
      setScanState((prev) => {
        const base = prev[pi] ?? { protocol, models: [], error: null, picked: [] };
        const known = new Set((config.providers[pi]?.models ?? []).map((m) => m.name.trim()));
        const models = resp.success ? resp.models : [];
        // 新模型默认勾选；已配置的模型排除在可勾选外
        const picked = models.filter((m) => !known.has(m.name)).map((m) => m.name);
        return {
          ...prev,
          [pi]: {
            ...base,
            scanning: false,
            models,
            picked,
            error: resp.success ? null : (resp.error ?? '扫描失败'),
          },
        };
      });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '扫描请求失败';
      setScanState((prev) => ({
        ...prev,
        [pi]: {
          ...(prev[pi] ?? { protocol, models: [], error: null, picked: [] }),
          scanning: false,
          error: msg,
        },
      }));
    }
  };

  const togglePicked = (pi: number, name: string) => {
    setScanState((prev) => {
      const st = prev[pi];
      if (!st) return prev;
      const picked = st.picked.includes(name)
        ? st.picked.filter((n) => n !== name)
        : [...st.picked, name];
      return { ...prev, [pi]: { ...st, picked } };
    });
  };

  const adoptPicked = (pi: number) => {
    const st = scanState[pi];
    if (!st) return;
    const models = st.models.filter((m) => st.picked.includes(m.name));
    if (models.length === 0) return;
    onAddScannedModels(pi, models);
    setScanState((prev) => {
      const cur = prev[pi];
      if (!cur) return prev;
      return { ...prev, [pi]: { ...cur, models: [], picked: [], error: null } };
    });
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
      {config.providers.map((p, pi) => {
        const st = scanState[pi] ?? {
          protocol: 'openai' as ProviderProtocol,
          scanning: false,
          models: [],
          error: null,
        };
        const isCollapsed = collapsed[pi] ?? true;
        return (
        <div
          key={p.name || pi}
          className="border border-[var(--color-border)] rounded-lg p-4 mb-3 space-y-3"
        >
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
              <Toggle
                checked={p.enabled}
                onChange={(v) => onUpdateProvider(pi, 'enabled', v)}
                label="启用"
              />
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
                    onChange={(e) =>
                      onUpdateProvider(pi, 'timeout', parseInt(e.target.value) || 60)
                    }
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

          {/* ── Provider discovery (scan models) ── */}
          <div className="pt-2 border-t border-[var(--color-border)]">
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => setCollapsed((prev) => ({ ...prev, [pi]: !(prev[pi] ?? true) }))}
                aria-label={`${p.name} ${isCollapsed ? '展开' : '收起'}模型列表`}
                aria-expanded={!isCollapsed}
                title={isCollapsed ? '展开模型列表' : '收起模型列表'}
                className="flex items-center gap-1 px-2 py-1.5 rounded-md text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors"
              >
                {isCollapsed ? <ChevronRight size={16} /> : <ChevronDown size={16} />}
                <span className="text-[10px] font-medium">{p.models.length}</span>
              </button>
              <select
                aria-label={`${p.name} 扫描协议`}
                value={st.protocol}
                onChange={(e) => {
                  const protocol = e.target.value as ProviderProtocol;
                  setScanState((prev) => ({
                    ...prev,
                    [pi]: prev[pi]
                      ? { ...prev[pi], protocol }
                      : { protocol, scanning: false, models: [], error: null, picked: [] },
                  }));
                }}
                className="px-2 py-1.5 text-xs rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              >
                <option value="openai">OpenAI 兼容</option>
                <option value="ollama">Ollama 原生</option>
              </select>
              <button
                onClick={() => handleScanModels(pi, p.endpoint, st.protocol, p.name)}
                disabled={st.scanning || !p.endpoint}
                title={!p.endpoint ? '请先填写端点 URL' : undefined}
                className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors disabled:opacity-50"
              >
                {st.scanning ? (
                  <Loader2 size={14} className="animate-spin" />
                ) : (
                  <RefreshCw size={14} />
                )}
                扫描模型
              </button>
              {p.name && (
                <span className="text-[10px] text-[var(--color-text-tertiary)]">
                  内置目录命中时零网络返回
                </span>
              )}
            </div>

            {/* Scan error */}
            {st.error && (
              <div className="mt-2 p-2 rounded-md bg-red-500/10 border border-red-500/30 text-xs text-red-400">
                {st.error}
              </div>
            )}

            {/* Discovered models (bulk adopt) */}
            {st.models.length > 0 && (
              <div className="mt-3">
                <div className="flex items-center justify-between mb-2">
                  <h4 className="text-xs font-medium text-[var(--color-text-primary)]">
                    已发现模型 ({st.models.length})
                  {st.picked.length > 0 && <span> · 新选 {st.picked.length}</span>}
                </h4>
                <button
                  onClick={() => adoptPicked(pi)}
                  disabled={st.picked.length === 0}
                  className="flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-md bg-accent text-white hover:bg-accent-hover transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                >
                  <Plus size={12} />
                  添加选中 {st.picked.length} 个
                </button>
              </div>
              <div className="space-y-1.5 mt-2">
                {st.models.map((m) => {
                  const already = (config.providers[pi]?.models ?? []).some(
                    (em) => em.name === m.name,
                  );
                  const checked = already || st.picked.includes(m.name);
                  return (
                    <div
                      key={m.name}
                      className={
                        'p-2 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)]' +
                        (already ? ' opacity-60' : '')
                      }
                    >
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={checked}
                            disabled={already}
                            onChange={() => togglePicked(pi, m.name)}
                            className="accent-accent shrink-0 cursor-pointer"
                            aria-label={`选择 ${m.name}`}
                          />
                          <span className="text-xs font-medium text-[var(--color-text-primary)] truncate">
                            {m.display_name ?? m.name}
                          </span>
                          {m.size && (
                            <span className="text-[10px] text-[var(--color-text-tertiary)] shrink-0">
                              {m.size}
                            </span>
                          )}
                        </div>
                        {m.capabilities.length > 0 && (
                          <div className="flex flex-wrap gap-1 mt-1">
                            {m.capabilities.map((cap) => (
                              <span
                                key={cap}
                                className="px-1.5 py-0.5 text-[10px] rounded-md bg-accent/10 text-accent border border-accent/20"
                              >
                                {cap}
                              </span>
                            ))}
                          </div>
                        )}
                        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 mt-1">
                          {(() => {
                            const specLine = formatDiscoveredSpecs(m);
                            if (specLine) {
                              return (
                                <span className="text-[10px] text-[var(--color-text-tertiary)]">
                                  {specLine}
                                </span>
                              );
                            }
                            return null;
                          })()}
                          {m.reasoning_efforts && m.reasoning_efforts.length > 0 && (
                            <span className="text-[10px] text-[var(--color-text-tertiary)]">
                              档位：{m.reasoning_efforts.join(', ')}
                            </span>
                          )}
                          {already && (
                            <span className="text-[10px] text-[var(--color-text-tertiary)]">
                              已配置
                            </span>
                          )}
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>
              </div>
            )}
          </div>

          {/* ── Models under this provider ── */}
          {!isCollapsed && (
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
              <div
                key={m.name || mi}
                className="flex items-start gap-2 mb-2 p-2 rounded bg-[var(--color-bg-secondary)]"
              >
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
                        className={`relative inline-flex items-center gap-1 px-1.5 py-0.5 text-xs rounded cursor-pointer border transition-colors ${
                          m.capabilities.includes(cap)
                            ? 'border-accent bg-accent-light text-accent'
                            : 'border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:border-[var(--color-text-tertiary)]'
                        }`}
                      >
                        <input
                          type="checkbox"
                          checked={m.capabilities.includes(cap)}
                          // 铺满 label：几何位置与可见标签重合，聚焦不会滚动页面
                          className="absolute inset-0 w-full h-full opacity-0 cursor-pointer"
                          onChange={() => onToggleModelCapability(pi, mi, cap)}
                        />
                        {CAPABILITY_LABELS[cap]}
                      </label>
                    ))}
                  </div>
                                    {/* 模型规格字段：按 capability 显示；留空 = 未配置 = 走后端内置模型表默认 */}
                  {m.capabilities.some((cap) => cap === 'chat' || cap === 'vision') && (
                    <div className="grid grid-cols-2 gap-1.5 mt-1.5">
                      <input
                        type="number"
                        min={1}
                        aria-label="上下文长度"
                        value={m.context_length ?? ''}
                        onChange={(e) =>
                          onUpdateModel(
                            pi,
                            mi,
                            'context_length',
                            e.target.value === '' ? undefined : parseInt(e.target.value, 10),
                          )
                        }
                        className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                        placeholder="默认 32768（内置表自动匹配）"
                      />
                      <input
                        type="number"
                        min={1}
                        aria-label="最大输出"
                        value={m.max_output_tokens ?? ''}
                        onChange={(e) =>
                          onUpdateModel(
                            pi,
                            mi,
                            'max_output_tokens',
                            e.target.value === '' ? undefined : parseInt(e.target.value, 10),
                          )
                        }
                        className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                        placeholder="默认 8192"
                      />
                    </div>
                  )}
                  {/* 思考强度档位（每个模型自己的值）：逗号分隔任意档位值，如 "low, high, max"；
                      留空 = 不支持思考（显示严格等于配置，无内置注入） */}
                  {m.capabilities.some((cap) => cap === 'chat' || cap === 'vision') && (
                    <div className="mt-1.5">
                      <EffortInput
                        value={(m.reasoning_efforts ?? []).join(', ')}
                        onChange={(efforts) => onUpdateModel(pi, mi, 'reasoning_efforts', efforts)}
                      />
                    </div>
                  )}
                  {m.capabilities.some(
                    (cap) => cap === 'text-embedding' || cap === 'multimodal-embedding',
                  ) && (
                    <input
                      type="number"
                      min={1}
                      aria-label="嵌入输入上限"
                      value={m.max_input_tokens ?? ''}
                      onChange={(e) =>
                        onUpdateModel(
                          pi,
                          mi,
                          'max_input_tokens',
                          e.target.value === '' ? undefined : parseInt(e.target.value, 10),
                        )
                      }
                      className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent mt-1.5"
                      placeholder="默认 8192"
                    />
                  )}
                  {/* 生效规格（只读；后端解析结果，key = "{provider}/{model}"；旧后端无数据时不渲染） */}
                  {(() => {
                    const spec = config.resolvedSpecs[`${p.name}/${m.name}`];
                    const cat = config.modelCatalog?.[`${p.name}/${m.name}`];
                    return spec ? (
                      <ResolvedSpecRow spec={spec} model={m} catalog={cat} />
                    ) : null;
                  })()}
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
          )}
        </div>
        );
      })}

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
