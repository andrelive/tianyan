import { FieldRow } from '@/components/ui/FieldRow';
import type { StepProps } from './wizard.types';

export default function AgentStep({
  data,
  onChange,
  onNext: _onNext,
  onBack: _onBack,
  errors: _errors,
}: StepProps) {
  return (
    <div>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">
        Agent 行为配置
      </h2>
      <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
        自定义 Agent 的基本行为方式。
      </p>

      <div className="space-y-5">
        <p className="text-xs text-[var(--color-text-tertiary)]">
          技能执行、记忆持久化与流式响应是智能体的固有能力，始终开启；
          思考模式在会话输入区按对话选择（仅对支持思考的模型生效）。
        </p>

        <FieldRow label="最大对话轮次" description="单次对话中 Agent 的最大推理步数">
          <input
            type="number"
            min={1}
            max={200}
            value={data.maxTurns}
            onChange={(e) => onChange({ maxTurns: parseInt(e.target.value) || 200 })}
            className="w-full max-w-xs px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            aria-label="最大对话轮次"
          />
        </FieldRow>
      </div>
    </div>
  );
}
