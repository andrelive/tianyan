import { FieldRow } from '@/components/ui/FieldRow';
import type { StepProps } from './wizard.types';

/** 搜索后端选项（与后端 SearchBackend::from_config 支持值对齐）。 */
const BACKENDS = [
  { value: '360', label: '360 搜索（默认，国内可达）' },
  { value: 'duckduckgo', label: 'DuckDuckGo（零配置）' },
  { value: 'searxng', label: 'SearXNG（自托管端点）' },
  { value: 'deepseek', label: 'DeepSeek 官方搜索（需 API Key）' },
];

export default function WebStep({
  data,
  onChange,
  onNext: _onNext,
  onBack: _onBack,
  errors: _errors,
}: StepProps) {
  const isSearxng = data.search_backend === 'searxng';
  const isDeepseek = data.search_backend === 'deepseek';
  return (
    <div>
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Web 搜索配置</h2>
      <p className="text-xs text-[var(--color-text-tertiary)] mb-5">
        web_search / web_fetch 工具的网络配置。默认 360 搜索（国内可达）； 也可切换 DuckDuckGo /
        SearXNG（自托管）或 DeepSeek 官方搜索（按次计费，需 API Key）。
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

        {isDeepseek && (
          <>
            <FieldRow label="DeepSeek API Key" description="必填（搜索按次计费，仅非空结果计费）">
              <input
                type="password"
                value={data.deepseek_api_key}
                onChange={(e) => onChange({ deepseek_api_key: e.target.value })}
                className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                placeholder="sk-..."
                aria-label="DeepSeek API Key"
              />
            </FieldRow>
            <FieldRow label="DeepSeek 端点（可选）" description="默认官方 https://api.deepseek.com">
              <input
                type="text"
                value={data.deepseek_endpoint}
                onChange={(e) => onChange({ deepseek_endpoint: e.target.value })}
                className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                placeholder="https://api.deepseek.com"
                aria-label="DeepSeek 端点"
              />
            </FieldRow>
          </>
        )}
      </div>
    </div>
  );
}
