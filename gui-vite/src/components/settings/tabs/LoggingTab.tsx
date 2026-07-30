import { Toggle, FieldRow, SectionTitle } from './shared';
import type { ConfigState } from '@/lib/types';

interface LoggingTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function LoggingTab({ config, onUpdateField }: LoggingTabProps) {
  return (
    <div>
      <SectionTitle title="日志设置" />
      <div className="grid grid-cols-2 gap-4">
        <FieldRow label="日志级别">
          <select
            value={config.log_level}
            onChange={(e) => onUpdateField('log_level', e.target.value)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          >
            <option value="debug">Debug</option>
            <option value="info">Info</option>
            <option value="warn">Warn</option>
            <option value="error">Error</option>
          </select>
        </FieldRow>
        <FieldRow label="日志格式">
          <select
            value={config.log_format}
            onChange={(e) => onUpdateField('log_format', e.target.value)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          >
            <option value="text">Text</option>
            <option value="json">JSON</option>
          </select>
        </FieldRow>
        <FieldRow label="最大文件大小 (MB)">
          <input
            type="number"
            min={1}
            value={config.log_max_file_size}
            onChange={(e) => onUpdateField('log_max_file_size', parseInt(e.target.value) || 10)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </FieldRow>
        <FieldRow label="最大文件数">
          <input
            type="number"
            min={1}
            max={100}
            value={config.log_max_files}
            onChange={(e) => onUpdateField('log_max_files', parseInt(e.target.value) || 5)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </FieldRow>
        <div className="flex items-center gap-6">
          <Toggle
            checked={config.log_include_timestamp}
            onChange={(v) => onUpdateField('log_include_timestamp', v)}
            label="包含时间戳"
          />
          <Toggle
            checked={config.log_include_location}
            onChange={(v) => onUpdateField('log_include_location', v)}
            label="包含位置信息"
          />
        </div>
      </div>
    </div>
  );
}
