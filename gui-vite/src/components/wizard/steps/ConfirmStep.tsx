import { Cpu, Database, Bot, Loader2 } from 'lucide-react';
import { MODEL_CAPABILITY_LABELS } from '@/lib/types';
import type { StepProps } from './wizard.types';

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
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">确认配置</h2>
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
                  value={data.modelCaps.map((c) => MODEL_CAPABILITY_LABELS[c]).join(', ') || '无'}
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
            <SummaryRow label="技能/记忆/流式" value="始终开启" />
            <SummaryRow label="思考模式" value="会话时选择" />
            <SummaryRow label="最大轮次" value={String(data.maxTurns)} />
          </div>
        </div>
      </div>

      {submitting && (
        <div
          className="flex items-center justify-center gap-2 mt-4 text-sm text-[var(--color-text-secondary)]"
          aria-live="polite"
        >
          <Loader2 size={16} className="animate-spin" />
          正在保存配置...
        </div>
      )}
    </div>
  );
}
