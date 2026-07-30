import { Cpu, Database, Bot, Loader2 } from 'lucide-react';
import type { ModelCapability } from '@/lib/types';
import type { StepProps } from './wizard.types';

const CAPABILITY_LABELS: Record<ModelCapability, string> = {
  chat: '对话 (Chat)',
  vision: '视觉 (Vision)',
  'text-embedding': '文本嵌入',
  'multimodal-embedding': '多模态嵌入',
};

interface SummaryRowProps {
  label: string;
  value: string;
}

function SummaryRow({ label, value }: SummaryRowProps) {
  return (
    <div className="flex items-center justify-between">
      <span className="text-[var(--color-text-secondary)]">{label}</span>
      <span className="text-[var(--color-text-primary)] font-mono text-xs max-w-[200px] truncate">
        {value}
      </span>
    </div>
  );
}

interface ConfirmStepProps extends StepProps {
  submitting: boolean;
}

export default function ConfirmStep({
  data,
  onChange: _onChange,
  onNext: _onNext,
  onBack: _onBack,
  errors: _errors,
  submitting,
}: ConfirmStepProps) {
  return (
    <div>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">
        确认配置
      </h2>
      <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
        请检查以下配置无误，点击"完成配置"开始使用天演。
      </p>

      <div className="space-y-3">
        {/* Model summary */}
        <div className="rounded-lg border border-[var(--color-border)] overflow-hidden">
          <div className="px-3 py-2 bg-[var(--color-bg-secondary)] text-sm font-medium text-[var(--color-text-primary)] flex items-center gap-2">
            <Cpu size={14} className="text-accent" />
            模型服务
          </div>
          <div className="px-3 py-2 space-y-1 text-sm">
            {data.providerName.trim() ? (
              <>
                <SummaryRow label="提供商" value={data.providerName} />
                <SummaryRow label="端点" value={data.providerEndpoint} />
                <SummaryRow
                  label="API Key"
                  value={data.providerApiKey ? `${data.providerApiKey.slice(0, 8)}...` : ''}
                />
                <SummaryRow label="模型" value={data.modelName || '(未设置)'} />
                <SummaryRow
                  label="能力标签"
                  value={data.modelCaps.map((c) => CAPABILITY_LABELS[c]).join(', ') || '无'}
                />
              </>
            ) : (
              <p className="text-[var(--color-text-tertiary)] text-xs">
                未配置模型服务（可在设置中补充）
              </p>
            )}
          </div>
        </div>

        {/* Data summary */}
        <div className="rounded-lg border border-[var(--color-border)] overflow-hidden">
          <div className="px-3 py-2 bg-[var(--color-bg-secondary)] text-sm font-medium text-[var(--color-text-primary)] flex items-center gap-2">
            <Database size={14} className="text-accent" />
            数据存储
          </div>
          <div className="px-3 py-2 space-y-1 text-sm">
            <SummaryRow label="数据目录" value={data.dataDir} />
            <SummaryRow label="向量维度" value={String(data.vectorDim)} />
            <SummaryRow label="向量数据库" value="LanceDB (嵌入式)" />
          </div>
        </div>

        {/* Agent summary */}
        <div className="rounded-lg border border-[var(--color-border)] overflow-hidden">
          <div className="px-3 py-2 bg-[var(--color-bg-secondary)] text-sm font-medium text-[var(--color-text-primary)] flex items-center gap-2">
            <Bot size={14} className="text-accent" />
            Agent 行为
          </div>
          <div className="px-3 py-2 space-y-1 text-sm">
            <SummaryRow label="技能" value={data.enableSkills ? '启用' : '禁用'} />
            <SummaryRow label="记忆" value={data.enableMemory ? '启用' : '禁用'} />
            <SummaryRow label="流式响应" value={data.streamResponses ? '启用' : '禁用'} />
            <SummaryRow label="思考过程" value={data.enableThinking ? '显示' : '隐藏'} />
            <SummaryRow label="最大轮次" value={String(data.maxTurns)} />
          </div>
        </div>
      </div>

      {submitting && (
        <div className="flex items-center justify-center gap-2 mt-4 text-sm text-[var(--color-text-secondary)]" aria-live="polite">
          <Loader2 size={16} className="animate-spin" />
          正在保存配置...
        </div>
      )}
    </div>
  );
}
