import type { StepProps } from './wizard.types';

function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
}) {
  return (
    <label className="inline-flex items-center gap-2 cursor-pointer group">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="sr-only peer"
        aria-label={label || ''}
      />
      <div className="relative w-10 h-5 rounded-full bg-[var(--color-bg-tertiary)] peer-checked:bg-accent transition-colors after:content-[''] after:absolute after:top-0.5 after:left-0.5 after:w-4 after:h-4 after:bg-white after:rounded-full after:shadow-sm after:transition-all peer-checked:after:translate-x-5" />
      {label && (
        <span className="text-sm text-[var(--color-text-secondary)]">{label}</span>
      )}
    </label>
  );
}

interface FieldRowProps {
  label: string;
  children: React.ReactNode;
  description?: string;
}

function FieldRow({ label, children, description }: FieldRowProps) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-sm font-medium text-[var(--color-text-primary)]">
        {label}
      </label>
      {children}
      {description && (
        <p className="text-xs text-[var(--color-text-tertiary)]">{description}</p>
      )}
    </div>
  );
}

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
        <div className="grid grid-cols-2 gap-4">
          <div className="flex items-center justify-between p-3 rounded-lg border border-[var(--color-border)]">
            <div>
              <p className="text-sm font-medium text-[var(--color-text-primary)]">启用技能</p>
              <p className="text-xs text-[var(--color-text-tertiary)]">允许 Agent 调用内置工具</p>
            </div>
            <Toggle
              checked={data.enableSkills}
              onChange={(v) => onChange({ enableSkills: v })}
            />
          </div>
          <div className="flex items-center justify-between p-3 rounded-lg border border-[var(--color-border)]">
            <div>
              <p className="text-sm font-medium text-[var(--color-text-primary)]">启用记忆</p>
              <p className="text-xs text-[var(--color-text-tertiary)]">Agent 可记住历史交互</p>
            </div>
            <Toggle
              checked={data.enableMemory}
              onChange={(v) => onChange({ enableMemory: v })}
            />
          </div>
          <div className="flex items-center justify-between p-3 rounded-lg border border-[var(--color-border)]">
            <div>
              <p className="text-sm font-medium text-[var(--color-text-primary)]">流式响应</p>
              <p className="text-xs text-[var(--color-text-tertiary)]">实时逐词显示回复</p>
            </div>
            <Toggle
              checked={data.streamResponses}
              onChange={(v) => onChange({ streamResponses: v })}
            />
          </div>
          <div className="flex items-center justify-between p-3 rounded-lg border border-[var(--color-border)]">
            <div>
              <p className="text-sm font-medium text-[var(--color-text-primary)]">思考过程</p>
              <p className="text-xs text-[var(--color-text-tertiary)]">显示 Agent 的推理步骤</p>
            </div>
            <Toggle
              checked={data.enableThinking}
              onChange={(v) => onChange({ enableThinking: v })}
            />
          </div>
        </div>

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
