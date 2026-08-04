import React from 'react';

/* ───────── Helper components (shared across tabs) ───────── */

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
    <label className="inline-flex items-center gap-2 cursor-pointer group">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="sr-only peer"
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

export function FieldRow({
  label,
  children,
  description,
}: {
  label: string;
  children: React.ReactNode;
  description?: string;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-sm font-medium text-[var(--color-text-primary)]">{label}</label>
      {children}
      {description && <p className="text-xs text-[var(--color-text-tertiary)]">{description}</p>}
    </div>
  );
}

export function SectionTitle({ title }: { title: string }) {
  return (
    <h3 className="text-base font-semibold text-[var(--color-text-primary)] border-b border-[var(--color-border)] pb-2 mb-4">
      {title}
    </h3>
  );
}
