import { Toggle, SectionTitle } from './shared';
import type { ConfigState } from '@/lib/types';

interface MemoryTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function MemoryTab({ config, onUpdateField }: MemoryTabProps) {
  return (
    <div>
      <SectionTitle title="记忆设置" />
      <div className="space-y-3">
        <p className="text-xs text-[var(--color-text-tertiary)]">
          记忆的写入与治理由演化任务（自演化综述）统一负责；此处仅保留巩固开关。
        </p>
        <div className="flex items-center gap-3">
          <Toggle
            checked={config.auto_consolidation}
            onChange={(v) => onUpdateField('auto_consolidation', v)}
            label="自动巩固（写前查重与合并归纳）"
          />
        </div>
      </div>
    </div>
  );
}
