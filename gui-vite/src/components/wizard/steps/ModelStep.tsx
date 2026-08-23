import { MODEL_CAPABILITIES, MODEL_CAPABILITY_LABELS, type ModelCapability } from '@/lib/types';
import type { StepProps } from './wizard.types';

interface FieldRowProps {
  label: string;
  children: React.ReactNode;
  description?: string;
}

function FieldRow({ label, children, description }: FieldRowProps) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-sm font-medium text-[var(--color-text-primary)]">{label}</label>
      {children}
      {description && <p className="text-xs text-[var(--color-text-tertiary)]">{description}</p>}
    </div>
  );
}

export default function ModelStep({
  data,
  onChange,
  onNext: _onNext,
  onBack: _onBack,
  errors: _errors,
}: StepProps) {
  const toggleCap = (cap: ModelCapability) => {
    const prev = data.modelCaps;
    const next = prev.includes(cap) ? prev.filter((c) => c !== cap) : [...prev, cap];
    onChange({ modelCaps: next });
  };

  return (
    <div>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">模型服务配置</h2>
      <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
        添加 AI 模型服务提供商及第一个模型。所有服务通过 OpenAI 兼容 API 通信。
      </p>

      <div className="space-y-4">
        <div className="grid grid-cols-2 gap-4">
          <FieldRow label="提供商名称">
            <input
              type="text"
              value={data.providerName}
              onChange={(e) => onChange({ providerName: e.target.value })}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="openai"
              aria-label="提供商名称"
            />
          </FieldRow>
          <FieldRow label="模型名称" description="如 gpt-4、deepseek-chat">
            <input
              type="text"
              value={data.modelName}
              onChange={(e) => onChange({ modelName: e.target.value })}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="gpt-4"
              aria-label="模型名称"
            />
          </FieldRow>
        </div>

        <FieldRow label="端点 URL">
          <input
            type="text"
            value={data.providerEndpoint}
            onChange={(e) => onChange({ providerEndpoint: e.target.value })}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            placeholder="https://api.openai.com/v1"
            aria-label="端点 URL"
          />
        </FieldRow>

        <FieldRow label="API Key" description="支持 ${ENV_VAR} 格式引用环境变量">
          <input
            type="password"
            value={data.providerApiKey}
            onChange={(e) => onChange({ providerApiKey: e.target.value })}
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
                  data.modelCaps.includes(cap)
                    ? 'border-accent bg-accent-light text-accent'
                    : 'border-[var(--color-border)] text-[var(--color-text-tertiary)] hover:border-[var(--color-text-tertiary)]'
                }`}
              >
                <input
                  type="checkbox"
                  checked={data.modelCaps.includes(cap)}
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
