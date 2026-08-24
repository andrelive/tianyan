import { useEffect, useRef, useState } from 'react';
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
import { Toggle, FieldRow, INPUT_CLASS } from './shared';
import type {
  ProviderConfigState,
  ProviderModelEntry,
  ModelCapability,
  ModelCatalogInfo,
  DiscoveredModelInfo,
  ProviderProtocol,
  ResolvedModelSpec,
} from '@/lib/types';
import { MODEL_CAPABILITIES, MODEL_CAPABILITY_LABELS } from '@/lib/types';
import type { ProviderScanState } from '@/hooks/use-provider-scan';

/* ── 数字紧凑缩写：>=1M → "1M"；>=1K → "128K"；否则原样。 ── */

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
    const tokens = raw
      .split(/[,，]/)
      .map((v) => v.trim())
      .filter(Boolean);
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
  const isChatOrVision = model.capabilities.some((cap) => cap === 'chat' || cap === 'vision');
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

/* ── 单条模型编辑行 ── */

interface ModelRowProps {
  model: ProviderModelEntry;
  modelIndex: number;
  resolvedSpec: ResolvedModelSpec | undefined;
  catalog: ModelCatalogInfo | undefined;
  onRemove: () => void;
  onUpdate: (field: keyof ProviderModelEntry, value: unknown) => void;
  onToggleCapability: (cap: ModelCapability) => void;
}

function ModelRow({
  model,
  modelIndex,
  resolvedSpec,
  catalog,
  onRemove,
  onUpdate,
  onToggleCapability,
}: ModelRowProps) {
  return (
    <div
      key={model.name || modelIndex}
      className="flex items-start gap-2 mb-2 p-2 rounded bg-[var(--color-bg-secondary)]"
    >
      <div className="flex-1 min-w-0">
        <input
          type="text"
          value={model.name}
          onChange={(e) => onUpdate('name', e.target.value)}
          className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent mb-1.5"
          placeholder="模型名称，如 gpt-4"
        />
        <div className="flex flex-wrap gap-1">
          {MODEL_CAPABILITIES.map((cap) => (
            <label
              key={cap}
              className={`relative inline-flex items-center gap-1 px-1.5 py-0.5 text-xs rounded cursor-pointer border transition-colors ${
                model.capabilities.includes(cap)
                  ? 'border-accent bg-accent-light text-accent'
                  : 'border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:border-[var(--color-text-tertiary)]'
              }`}
            >
              <input
                type="checkbox"
                checked={model.capabilities.includes(cap)}
                // 铺满 label：几何位置与可见标签重合，聚焦不会滚动页面
                className="absolute inset-0 w-full h-full opacity-0 cursor-pointer"
                onChange={() => onToggleCapability(cap)}
              />
              {MODEL_CAPABILITY_LABELS[cap]}
            </label>
          ))}
        </div>
        {/* 模型规格字段：按 capability 显示；留空 = 未配置 = 走后端内置模型表默认 */}
        {model.capabilities.some((cap) => cap === 'chat' || cap === 'vision') && (
          <div className="grid grid-cols-2 gap-1.5 mt-1.5">
            <input
              type="number"
              min={1}
              aria-label="上下文长度"
              value={model.context_length ?? ''}
              onChange={(e) =>
                onUpdate(
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
              value={model.max_output_tokens ?? ''}
              onChange={(e) =>
                onUpdate(
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
        {model.capabilities.some((cap) => cap === 'chat' || cap === 'vision') && (
          <div className="mt-1.5">
            <EffortInput
              value={(model.reasoning_efforts ?? []).join(', ')}
              onChange={(efforts) => onUpdate('reasoning_efforts', efforts)}
            />
          </div>
        )}
        {model.capabilities.some(
          (cap) => cap === 'text-embedding' || cap === 'multimodal-embedding',
        ) && (
          <input
            type="number"
            min={1}
            aria-label="嵌入输入上限"
            value={model.max_input_tokens ?? ''}
            onChange={(e) =>
              onUpdate(
                'max_input_tokens',
                e.target.value === '' ? undefined : parseInt(e.target.value, 10),
              )
            }
            className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent mt-1.5"
            placeholder="默认 8192"
          />
        )}
        {/* 生效规格（只读；后端解析结果，key = "{provider}/{model}"；旧后端无数据时不渲染） */}
        {resolvedSpec && <ResolvedSpecRow spec={resolvedSpec} model={model} catalog={catalog} />}
      </div>
      <button
        onClick={onRemove}
        className="p-1 rounded text-[var(--color-text-tertiary)] hover:text-[var(--color-error)] hover:bg-[var(--color-error-bg)] transition-colors shrink-0"
        title="删除模型"
      >
        <Trash2 size={12} />
      </button>
    </div>
  );
}

/* ── 扫描发现区（协议选择 + 扫描 + 结果勾选/批量采纳） ── */

interface ScanSectionProps {
  providerName: string;
  endpoint: string;
  modelCount: number;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  scanState: ProviderScanState;
  knownModels: ProviderModelEntry[];
  onScan: (endpoint: string, protocol: ProviderProtocol, providerName: string) => void;
  onSetScanProtocol: (protocol: ProviderProtocol) => void;
  onTogglePicked: (name: string) => void;
  onAdoptPicked: () => void;
}

function ScanSection({
  providerName,
  endpoint,
  modelCount,
  collapsed: isCollapsed,
  onToggleCollapsed,
  scanState,
  knownModels,
  onScan,
  onSetScanProtocol,
  onTogglePicked,
  onAdoptPicked,
}: ScanSectionProps) {
  const st = scanState;
  return (
    <div className="pt-2 border-t border-[var(--color-border)]">
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={onToggleCollapsed}
          aria-label={`${providerName} ${isCollapsed ? '展开' : '收起'}模型列表`}
          aria-expanded={!isCollapsed}
          title={isCollapsed ? '展开模型列表' : '收起模型列表'}
          className="flex items-center gap-1 px-2 py-1.5 rounded-md text-[var(--color-text-tertiary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors"
        >
          {isCollapsed ? <ChevronRight size={16} /> : <ChevronDown size={16} />}
          <span className="text-[10px] font-medium">{modelCount}</span>
        </button>
        <select
          aria-label={`${providerName} 扫描协议`}
          value={st.protocol}
          onChange={(e) => onSetScanProtocol(e.target.value as ProviderProtocol)}
          className="ml-auto px-2 py-1.5 text-xs rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
        >
          <option value="openai">OpenAI 兼容</option>
          <option value="ollama">Ollama 原生</option>
        </select>
        <button
          onClick={() => onScan(endpoint, st.protocol, providerName)}
          disabled={st.scanning || !endpoint}
          title={!endpoint ? '请先填写端点 URL' : undefined}
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors disabled:opacity-50"
        >
          {st.scanning ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
          扫描模型
        </button>
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
              onClick={onAdoptPicked}
              disabled={st.picked.length === 0}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-md bg-accent text-white hover:bg-accent-hover transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              <Plus size={12} />
              添加选中 {st.picked.length} 个
            </button>
          </div>
          <div className="space-y-1.5 mt-2">
            {st.models.map((m) => {
              const already = knownModels.some((em) => em.name === m.name);
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
                        onChange={() => onTogglePicked(m.name)}
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
  );
}

/* ── Provider 卡片（名称/端点/启用/密钥/测试 + 扫描 + 模型列表） ── */

export interface ProviderCardProps {
  provider: ProviderConfigState;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  testStatus: 'idle' | 'testing' | 'success' | 'error';
  scanState: ProviderScanState;
  resolvedSpecs: Record<string, ResolvedModelSpec>;
  modelCatalog: Record<string, ModelCatalogInfo>;
  onUpdateProvider: (field: keyof ProviderConfigState, value: unknown) => void;
  onRemoveProvider: () => void;
  onTestConnection: () => void;
  onScan: (endpoint: string, protocol: ProviderProtocol, providerName: string) => void;
  onSetScanProtocol: (protocol: ProviderProtocol) => void;
  onTogglePicked: (name: string) => void;
  onAdoptPicked: () => void;
  onAddModel: () => void;
  onRemoveModel: (modelIndex: number) => void;
  onUpdateModel: (modelIndex: number, field: keyof ProviderModelEntry, value: unknown) => void;
  onToggleModelCapability: (modelIndex: number, cap: ModelCapability) => void;
}

export default function ProviderCard({
  provider: p,
  collapsed: isCollapsed,
  onToggleCollapsed,
  testStatus,
  scanState,
  resolvedSpecs,
  modelCatalog,
  onUpdateProvider,
  onRemoveProvider,
  onTestConnection,
  onScan,
  onSetScanProtocol,
  onTogglePicked,
  onAdoptPicked,
  onAddModel,
  onRemoveModel,
  onUpdateModel,
  onToggleModelCapability,
}: ProviderCardProps) {
  return (
    <div className="border border-[var(--color-border)] rounded-lg p-4 mb-3 space-y-3">
      <div className="grid grid-cols-[1fr_1fr_auto_auto] gap-3 items-end">
        <FieldRow label="名称">
          <input
            type="text"
            value={p.name}
            onChange={(e) => onUpdateProvider('name', e.target.value)}
            className={INPUT_CLASS}
            placeholder="openai"
          />
        </FieldRow>
        <FieldRow label="端点 URL">
          <input
            type="text"
            value={p.endpoint}
            onChange={(e) => onUpdateProvider('endpoint', e.target.value)}
            className={INPUT_CLASS}
            placeholder="https://api.openai.com/v1"
          />
        </FieldRow>
        <div className="flex items-center gap-2 pb-1">
          <Toggle
            checked={p.enabled}
            onChange={(v) => onUpdateProvider('enabled', v)}
            label="启用"
          />
        </div>
        <button
          onClick={onRemoveProvider}
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
            onChange={(e) => onUpdateProvider('api_key', e.target.value)}
            className={INPUT_CLASS}
            placeholder="sk-... 或 ${ENV_VAR}"
          />
        </FieldRow>
        <div className="flex items-end">
          <button
            onClick={onTestConnection}
            disabled={testStatus === 'testing'}
            className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors disabled:opacity-50"
          >
            {testStatus === 'testing' ? (
              <Loader2 size={14} className="animate-spin" />
            ) : testStatus === 'success' ? (
              <Check size={14} className="text-[var(--color-success)]" />
            ) : testStatus === 'error' ? (
              <X size={14} className="text-[var(--color-error)]" />
            ) : (
              <TestTube size={14} />
            )}
            测试连接
          </button>
        </div>
      </div>

      {/* ── Provider discovery (scan models) + 模型列表折叠开关 ── */}
      <ScanSection
        providerName={p.name}
        endpoint={p.endpoint}
        modelCount={p.models.length}
        collapsed={isCollapsed}
        onToggleCollapsed={onToggleCollapsed}
        scanState={scanState}
        knownModels={p.models}
        onScan={onScan}
        onSetScanProtocol={onSetScanProtocol}
        onTogglePicked={onTogglePicked}
        onAdoptPicked={onAdoptPicked}
      />

      {/* ── Models under this provider ── */}
      {!isCollapsed && (
        <div className="pt-2 border-t border-[var(--color-border)]">
          <div className="flex items-center justify-between mb-2">
            <span className="text-sm font-medium text-[var(--color-text-primary)]">模型列表</span>
            <button
              onClick={onAddModel}
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
            <ModelRow
              key={m.name || mi}
              model={m}
              modelIndex={mi}
              resolvedSpec={resolvedSpecs[`${p.name}/${m.name}`]}
              catalog={modelCatalog[`${p.name}/${m.name}`]}
              onRemove={() => onRemoveModel(mi)}
              onUpdate={(field, value) => onUpdateModel(mi, field, value)}
              onToggleCapability={(cap) => onToggleModelCapability(mi, cap)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
