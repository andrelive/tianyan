import { Toggle, SliderField, FieldRow, SectionTitle } from './shared';
import type { ConfigState } from '@/lib/types';

interface RetrievalTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function RetrievalTab({ config, onUpdateField }: RetrievalTabProps) {
  return (
    <div>
      <SectionTitle title="检索设置" />
      <div className="grid grid-cols-2 gap-4">
        <FieldRow label="检索 Top-K">
          <input
            type="number"
            min={1}
            max={100}
            value={config.retrieval_top_k}
            onChange={(e) => onUpdateField('retrieval_top_k', parseInt(e.target.value) || 10)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </FieldRow>
        <FieldRow label="最低分数">
          <SliderField
            value={config.min_score}
            onChange={(v) => onUpdateField('min_score', v)}
            min={0}
            max={1}
          />
        </FieldRow>
        <div className="flex items-center gap-3">
          <Toggle
            checked={config.two_stage_retrieval}
            onChange={(v) => onUpdateField('two_stage_retrieval', v)}
            label="两阶段检索"
          />
        </div>
        <FieldRow label="L0 乘数">
          <input
            type="number"
            min={1}
            max={10}
            value={config.l0_multiplier}
            onChange={(e) => onUpdateField('l0_multiplier', parseInt(e.target.value) || 3)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </FieldRow>
        <FieldRow label="最大上下文 Token">
          <input
            type="number"
            min={256}
            max={32000}
            step={256}
            value={config.max_context_tokens}
            onChange={(e) => onUpdateField('max_context_tokens', parseInt(e.target.value) || 4000)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </FieldRow>
        <div className="flex items-center gap-3">
          <Toggle
            checked={config.enable_cache}
            onChange={(v) => onUpdateField('enable_cache', v)}
            label="启用缓存"
          />
        </div>
        <FieldRow label="缓存 TTL (秒)">
          <input
            type="number"
            min={1}
            value={config.cache_ttl}
            onChange={(e) => onUpdateField('cache_ttl', parseInt(e.target.value) || 300)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </FieldRow>
      </div>
    </div>
  );
}
