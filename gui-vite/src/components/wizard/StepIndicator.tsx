import { Check, type LucideIcon } from 'lucide-react';

interface StepDef {
  id: string;
  label: string;
  icon: LucideIcon;
}

export default function StepIndicator({
  current,
  steps,
}: {
  current: number;
  steps: StepDef[];
}) {
  return (
    <nav aria-label="配置步骤" className="flex items-center justify-center gap-0">
      {steps.map((step, i) => {
        const isCompleted = i < current;
        const isCurrent = i === current;
        const Icon = step.icon;

        return (
          <div
            key={step.id}
            className="flex items-center"
            aria-current={isCurrent ? 'step' : undefined}
          >
            <div className="flex flex-col items-center">
              <div
                className={`w-9 h-9 rounded-full flex items-center justify-center text-sm font-medium transition-colors ${
                  isCompleted
                    ? 'bg-accent text-white'
                    : isCurrent
                      ? 'bg-accent text-white ring-2 ring-offset-2 ring-accent'
                      : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)]'
                }`}
                aria-label={`步骤 ${i + 1}: ${step.label}`}
              >
                {isCompleted ? <Check size={16} /> : <Icon size={16} />}
              </div>
              <span
                className={`mt-1.5 text-xs whitespace-nowrap ${
                  isCurrent
                    ? 'text-accent font-medium'
                    : isCompleted
                      ? 'text-[var(--color-text-secondary)]'
                      : 'text-[var(--color-text-tertiary)]'
                }`}
              >
                {step.label}
              </span>
            </div>

            {i < steps.length - 1 && (
              <div
                className={`w-16 h-0.5 mx-2 mb-5 rounded ${
                  i < current ? 'bg-accent' : 'bg-[var(--color-bg-tertiary)]'
                }`}
              />
            )}
          </div>
        );
      })}
    </nav>
  );
}
