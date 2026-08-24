import { Toggle, SliderField, FieldRow, SectionTitle, INPUT_CLASS } from './shared';
import type { ConfigState } from '@/lib/types';

interface MemoryTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function MemoryTab({ config, onUpdateField }: MemoryTabProps) {
  return (
    <div>
      <SectionTitle title="记忆设置" />
      <div className="grid grid-cols-2 gap-4">
        <FieldRow label="会话记忆上限">
          <input
            type="number"
            min={1}
            value={config.max_session_memory}
            onChange={(e) => onUpdateField('max_session_memory', parseInt(e.target.value) || 8000)}
            className={INPUT_CLASS}
          />
        </FieldRow>
        <FieldRow label="长期记忆上限">
          <input
            type="number"
            min={1}
            value={config.max_long_term_memory}
            onChange={(e) =>
              onUpdateField('max_long_term_memory', parseInt(e.target.value) || 10000)
            }
            className={INPUT_CLASS}
          />
        </FieldRow>
        <FieldRow label="重要性阈值">
          <SliderField
            value={config.importance_threshold}
            onChange={(v) => onUpdateField('importance_threshold', v)}
            min={0}
            max={1}
          />
        </FieldRow>
        <FieldRow label="衰减率">
          <SliderField
            value={config.decay_rate}
            onChange={(v) => onUpdateField('decay_rate', v)}
            min={0}
            max={1}
          />
        </FieldRow>
        <div className="flex items-center gap-3">
          <Toggle
            checked={config.auto_consolidation}
            onChange={(v) => onUpdateField('auto_consolidation', v)}
            label="自动整合"
          />
        </div>
        {config.auto_consolidation && (
          <FieldRow label="整合间隔 (秒)">
            <input
              type="number"
              min={60}
              value={config.consolidation_interval}
              onChange={(e) =>
                onUpdateField('consolidation_interval', parseInt(e.target.value) || 3600)
              }
              className={INPUT_CLASS}
            />
          </FieldRow>
        )}
      </div>
    </div>
  );
}
