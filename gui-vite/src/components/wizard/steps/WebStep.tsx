import { FieldRow } from '@/components/ui/FieldRow';
import type { StepProps } from './wizard.types';

/** 搜索后端选项（与后端 SearchBackend::from_config 支持值对齐）。 */
const BACKENDS = [
  { value: 'duckduckgo', label: 'DuckDuckGo（默认，零配置）' },
  { value: 'bing', label: 'Bing（国内可达）' },
  { value: 'searxng', label: 'SearXNG（自托管端点）' },
];

export default function WebStep({
  data,
  onChange,
  onNext: _onNext,
  onBack: _onBack,
  errors: _errors,
}: StepProps) {
  const isSearxng = data.search_backend === 'searxng';
  return (
    <div>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Web 搜索配置</h2>
      <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
        web_search / web_fetch 工具的网络配置。默认 DuckDuckGo 零配置可用； 国内网络不可达时可切换
        Bing（cn.bing.com 可达）或自托管 SearXNG。
      </p>

      <div className="space-y-4">
        <FieldRow label="搜索后端">
          <select
            value={data.search_backend}
            onChange={(e) => onChange({ search_backend: e.target.value })}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            aria-label="搜索后端"
          >
            {BACKENDS.map((b) => (
              <option key={b.value} value={b.value}>
                {b.label}
              </option>
            ))}
          </select>
        </FieldRow>

        {isSearxng && (
          <FieldRow label="SearXNG 端点" description="必填，如 https://searx.be">
            <input
              type="text"
              value={data.searxng_endpoint}
              onChange={(e) => onChange({ searxng_endpoint: e.target.value })}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="https://searx.be"
              aria-label="SearXNG 端点"
            />
          </FieldRow>
        )}
      </div>
    </div>
  );
}
