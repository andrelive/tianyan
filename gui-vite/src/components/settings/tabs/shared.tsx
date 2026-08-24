import { FieldRow } from '@/components/ui/FieldRow';

/* ───────── Helper components (shared across tabs) ─────────
   FieldRow 原语收敛至 components/ui/FieldRow（settings 与 wizard 共用，E4）。 */

export { FieldRow };

export function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
}) {
  return (
    <label className="relative inline-flex items-center gap-2 cursor-pointer group">
      <input
        type="checkbox"
        checked={checked}
        // 铺满 label：几何位置与可见标签重合，聚焦不会滚动页面
        className="absolute inset-0 w-full h-full opacity-0 cursor-pointer peer"
        onChange={(e) => onChange(e.target.checked)}
      />
      <div className="relative w-10 h-5 rounded-full bg-[var(--color-bg-tertiary)] peer-checked:bg-accent transition-colors after:content-[''] after:absolute after:top-0.5 after:left-0.5 after:w-4 after:h-4 after:bg-white after:rounded-full after:shadow-sm after:transition-all peer-checked:after:translate-x-5" />
      {label && <span className="text-sm text-[var(--color-text-secondary)]">{label}</span>}
    </label>
  );
}

export function SliderField({
  value,
  onChange,
  min,
  max,
  step,
  label,
}: {
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  step?: number;
  label?: string;
}) {
  return (
    <div className="flex items-center gap-3">
      {label && (
        <span className="text-xs text-[var(--color-text-tertiary)] w-16 shrink-0">{label}</span>
      )}
      <input
        type="range"
        min={min}
        max={max}
        step={step ?? 0.01}
        value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        className="flex-1 h-1.5 rounded-full appearance-none cursor-pointer bg-[var(--color-bg-tertiary)] accent-accent [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-3.5 [&::-webkit-slider-thumb]:h-3.5 [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-accent [&::-webkit-slider-thumb]:shadow-sm"
      />
      <span className="text-xs text-[var(--color-text-secondary)] w-8 text-right tabular-nums">
        {value.toFixed(2)}
      </span>
    </div>
  );
}

export const INPUT_CLASS =
  'w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent';

/** 统一文本输入（settings tabs 共用样式单点；原 32 处重复 className 收敛）。 */
export function TextInput({
  value,
  onChange,
  placeholder,
  ariaLabel,
  type = 'text',
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  ariaLabel?: string;
  type?: 'text' | 'password';
}) {
  return (
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      aria-label={ariaLabel}
      className={INPUT_CLASS}
    />
  );
}

/** 统一数字输入（min/max 钳制 + 解析失败回退 fallback）。 */
export function NumberInput({
  value,
  onChange,
  min,
  max,
  step,
  fallback,
  ariaLabel,
}: {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  fallback: number;
  ariaLabel?: string;
}) {
  return (
    <input
      type="number"
      min={min}
      max={max}
      step={step}
      value={value}
      onChange={(e) => onChange(parseInt(e.target.value) || fallback)}
      aria-label={ariaLabel}
      className={INPUT_CLASS}
    />
  );
}

export function SectionTitle({ title }: { title: string }) {
  return (
    <h3 className="text-base font-semibold text-[var(--color-text-primary)] border-b border-[var(--color-border)] pb-2 mb-4">
      {title}
    </h3>
  );
}
