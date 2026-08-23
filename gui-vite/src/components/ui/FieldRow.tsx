import type { ReactNode } from 'react';

/**
 * FieldRow —— 表单字段行原语（label + 控件 + 可选描述）。
 * 统一 settings 与 wizard 的字段外观；样式改动只此一处（E4 收敛 ×4 拷贝）。
 */
export function FieldRow({
  label,
  children,
  description,
}: {
  label: string;
  children: ReactNode;
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
