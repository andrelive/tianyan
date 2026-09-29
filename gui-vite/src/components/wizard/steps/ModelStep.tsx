import { useEffect, useState } from 'react';
import { FieldRow } from '@/components/ui/FieldRow';
import { emptyProvider } from '@/lib/config-transform';
import {
  MODEL_CAPABILITIES,
  MODEL_CAPABILITY_LABELS,
  type ModelCapability,
  type ProviderPresetInfo,
} from '@/lib/types';
import { fetchProviderPresets } from '@/lib/api-client';
import { firstModel, firstProvider, type StepProps } from './wizard.types';

export default function ModelStep({
  data,
  onChange,
  onNext: _onNext,
  onBack: _onBack,
  errors: _errors,
}: StepProps) {
  const provider = firstProvider(data);
  const model = firstModel(data);

  /* 预置服务商（ADR-046）：挂载时懒加载一次 → 选择后一键填 name + endpoint */
  const [presets, setPresets] = useState<ProviderPresetInfo[]>([]);
  const [presetId, setPresetId] = useState('');

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const resp = await fetchProviderPresets();
        if (!cancelled) setPresets(resp.presets);
      } catch {
        // 预置加载失败：向导仍可手工填写全部字段（静默降级）
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  /** 更新第一个提供商（保持现有模型数组；向导只编辑 models[0]）。 */
  const updateProvider = (updates: Partial<ReturnType<typeof emptyProvider>>) => {
    const next = { ...provider, ...updates } as typeof provider;
    onChange({ providers: [next, ...data.providers.slice(1)] });
  };

  /** 更新第一个模型（capabilities 变化同样写回；preferences 由收尾派生）。 */
  const updateModel = (updates: Partial<{ name: string; capabilities: ModelCapability[] }>) => {
    const model0 = provider.models[0] ?? { name: '', capabilities: ['chat' as ModelCapability] };
    const nextModel = { ...model0, ...updates };
    updateProvider({ models: [nextModel, ...provider.models.slice(1)] });
  };

  const toggleCap = (cap: ModelCapability) => {
    const prev = model.capabilities;
    const next = prev.includes(cap) ? prev.filter((c) => c !== cap) : [...prev, cap];
    updateModel({ capabilities: next });
  };

  /** 选择预置：填入提供商名与端点（其余字段保持用户已填内容）。 */
  const applyPreset = (id: string) => {
    setPresetId(id);
    const preset = presets.find((p) => p.id === id);
    if (!preset) return;
    updateProvider({ name: preset.id, endpoint: preset.endpoint });
  };

  return (
    <div>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">模型服务配置</h2>
      <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
        添加 AI 模型服务提供商及第一个模型。所有服务通过 OpenAI 兼容 API 通信。
      </p>

      <div className="space-y-4">
        <FieldRow label="预置服务商" description="选择后自动填入名称与端点（仍可修改）">
          <select
            value={presetId}
            onChange={(e) => applyPreset(e.target.value)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            aria-label="预置服务商"
          >
            <option value="">自定义（手动填写）</option>
            {presets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.display_name}
              </option>
            ))}
          </select>
        </FieldRow>
        <div className="grid grid-cols-2 gap-4">
          <FieldRow label="提供商名称">
            <input
              type="text"
              value={provider.name}
              onChange={(e) => updateProvider({ name: e.target.value })}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="openai"
              aria-label="提供商名称"
            />
          </FieldRow>
          <FieldRow label="模型名称" description="如 gpt-4、deepseek-chat">
            <input
              type="text"
              value={model.name}
              onChange={(e) => updateModel({ name: e.target.value })}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="gpt-4"
              aria-label="模型名称"
            />
          </FieldRow>
        </div>

        <FieldRow label="端点 URL">
          <input
            type="text"
            value={provider.endpoint}
            onChange={(e) => updateProvider({ endpoint: e.target.value })}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            placeholder="https://api.openai.com/v1"
            aria-label="端点 URL"
          />
        </FieldRow>

        <FieldRow label="API Key" description="支持 ${ENV_VAR} 格式引用环境变量">
          <input
            type="password"
            value={provider.api_key}
            onChange={(e) => updateProvider({ api_key: e.target.value })}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            placeholder="sk-... 或 ${OPENAI_API_KEY}"
            aria-label="API Key"
          />
        </FieldRow>

        <FieldRow label="模型能力标签" description="选择此模型支持的功能类型（可多选）">
          <div className="flex flex-wrap gap-2">
            {MODEL_CAPABILITIES.map((cap) => (
              <label
                key={cap}
                className={`relative inline-flex items-center gap-1 px-2.5 py-1 text-xs rounded-md cursor-pointer border transition-colors ${
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
                  onChange={() => toggleCap(cap)}
                />
                {MODEL_CAPABILITY_LABELS[cap]}
              </label>
            ))}
          </div>
        </FieldRow>
      </div>
    </div>
  );
}
