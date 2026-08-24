import { Toggle, FieldRow, NumberInput, SectionTitle, TextInput, INPUT_CLASS } from './shared';
import type { ConfigState } from '@/lib/types';

interface StorageTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function StorageTab({ config, onUpdateField }: StorageTabProps) {
  return (
    <div>
      <SectionTitle title="数据存储" />
      <div className="grid grid-cols-2 gap-4">
        <FieldRow
          label="数据目录"
          description="数据库（tianyan.db）、向量索引、快照等所有本地数据的存储路径"
        >
          <TextInput
            value={config.data_dir}
            onChange={(v) => onUpdateField('data_dir', v)}
            placeholder="~/.local/share/tianyan"
          />
        </FieldRow>
        <FieldRow label="集合名称" description="向量库中的集合/表名">
          <TextInput
            value={config.collection_name}
            onChange={(v) => onUpdateField('collection_name', v)}
          />
        </FieldRow>
        <FieldRow label="向量维度" description="向量嵌入的维度数，通常取决于使用的模型">
          <select
            value={config.vector_dimension}
            onChange={(e) => onUpdateField('vector_dimension', parseInt(e.target.value))}
            className={INPUT_CLASS}
          >
            <option value="384">384</option>
            <option value="768">768</option>
            <option value="1024">1024</option>
            <option value="1536">1536 (OpenAI ada-002 / text-embedding-3-small)</option>
            <option value="3072">3072 (text-embedding-3-large)</option>
          </select>
        </FieldRow>
        <FieldRow label="最大存储 (字节)">
          <NumberInput
            value={config.max_storage_size}
            onChange={(v) => onUpdateField('max_storage_size', v)}
            min={0}
            fallback={0}
          />
        </FieldRow>
        <div className="flex items-end pb-1">
          <div className="flex items-center gap-4">
            <Toggle
              checked={config.auto_cleanup}
              onChange={(v) => onUpdateField('auto_cleanup', v)}
              label="自动清理"
            />
            {config.auto_cleanup && (
              <div className="flex items-center gap-2">
                <label className="text-xs text-[var(--color-text-secondary)]">清理天数</label>
                <input
                  type="number"
                  min={1}
                  value={config.cleanup_days}
                  onChange={(e) => onUpdateField('cleanup_days', parseInt(e.target.value) || 365)}
                  className="w-20 px-2 py-1 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
