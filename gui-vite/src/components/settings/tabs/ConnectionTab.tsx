import { FieldRow, SectionTitle } from './shared';
import { useAppStore } from '@/lib/store';

export default function ConnectionTab() {
  const apiBaseUrl = useAppStore((s) => s.apiBaseUrl);
  const setApiBaseUrl = useAppStore((s) => s.setApiBaseUrl);
  const showToast = useAppStore((s) => s.showToast);

  return (
    <div>
      <SectionTitle title="连接设置" />
      <FieldRow label="API 基础地址" description="后端服务地址，修改后立即生效">
        <div className="flex gap-2">
          <input
            type="text"
            value={apiBaseUrl}
            onChange={(e) => setApiBaseUrl(e.target.value)}
            className="flex-1 px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            placeholder="http://localhost:3000"
          />
          <button
            onClick={() => {
              fetch(`${apiBaseUrl}/health`, { signal: AbortSignal.timeout(3000) })
                .then((r) => {
                  if (r.ok) showToast('连接成功', 'success');
                  else showToast(`服务返回 ${r.status}`, 'error');
                })
                .catch(() => showToast('无法连接到服务', 'error'));
            }}
            className="px-3 py-1.5 text-sm rounded-md bg-accent text-white hover:bg-accent-hover transition-colors"
          >
            测试连接
          </button>
        </div>
      </FieldRow>
    </div>
  );
}
