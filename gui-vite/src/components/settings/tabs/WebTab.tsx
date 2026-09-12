import { FieldRow, NumberInput, SectionTitle, TextInput, Toggle } from './shared';
import type { ConfigState } from '@/lib/types';

interface WebTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

/** 搜索后端选项（与后端 SearchBackend::from_config 支持值对齐）。 */
const BACKENDS = [
  { value: 'duckduckgo', label: 'DuckDuckGo（默认，零配置）' },
  { value: 'bing', label: 'Bing（国内可达）' },
  { value: 'searxng', label: 'SearXNG（自托管端点）' },
];

export default function WebTab({ config, onUpdateField }: WebTabProps) {
  const isSearxng = config.search_backend === 'searxng';
  return (
    <div>
      <SectionTitle title="Web 搜索" />
      <div className="space-y-4">
        <p className="text-xs text-[var(--color-text-tertiary)]">
          web_search / web_fetch 工具的网络配置。默认 DuckDuckGo 零配置可用； 国内网络不可达时可切换
          Bing（cn.bing.com 可达）或自托管 SearXNG。
        </p>

        <FieldRow label="启用 Web 工具" description="关闭后 web_search / web_fetch 不可用">
          <Toggle
            checked={config.web_enabled}
            onChange={(v) => onUpdateField('web_enabled', v)}
            label={config.web_enabled ? '已启用' : '已禁用'}
          />
        </FieldRow>

        <FieldRow label="搜索后端" description="切换后立即生效（工具按配置重建）">
          <select
            value={config.search_backend}
            onChange={(e) => onUpdateField('search_backend', e.target.value)}
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
          <FieldRow
            label="SearXNG 端点"
            description="必填，如 https://searx.be（支持 JSON API 的实例）"
          >
            <TextInput
              value={config.searxng_endpoint}
              onChange={(v) => onUpdateField('searxng_endpoint', v)}
              placeholder="https://searx.be"
              ariaLabel="SearXNG 端点"
            />
          </FieldRow>
        )}

        <div className="grid grid-cols-2 gap-4 pt-2">
          <FieldRow label="请求超时（秒）">
            <NumberInput
              value={config.web_timeout_secs}
              onChange={(v) => onUpdateField('web_timeout_secs', v)}
              min={1}
              max={120}
              fallback={15}
            />
          </FieldRow>
          <FieldRow label="缓存 TTL（秒）" description="搜索/抓取结果缓存时长">
            <NumberInput
              value={config.web_cache_ttl_secs}
              onChange={(v) => onUpdateField('web_cache_ttl_secs', v)}
              min={0}
              max={86400}
              fallback={600}
            />
          </FieldRow>
          <FieldRow label="抓取大小上限（字节）" description="单次 web_fetch 响应大小上限">
            <NumberInput
              value={config.web_max_fetch_bytes}
              onChange={(v) => onUpdateField('web_max_fetch_bytes', v)}
              min={1024}
              max={50 * 1024 * 1024}
              fallback={2 * 1024 * 1024}
            />
          </FieldRow>
        </div>
      </div>
    </div>
  );
}
