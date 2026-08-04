import { useState } from 'react';
import { Loader2, Wifi, WifiOff, RefreshCw, Plus } from 'lucide-react';
import { useAppStore } from '@/lib/store';
import { apiPost, scanOllamaModels, testOllamaConnection } from '@/lib/api-client';
import type { OllamaModelInfo } from '@/lib/types';
import { FieldRow, SectionTitle } from './shared';

type ConnectionStatus = 'idle' | 'testing' | 'connected' | 'disconnected';

export default function OllamaTab() {
  const showToast = useAppStore((s) => s.showToast);

  const [endpoint, setEndpoint] = useState('http://localhost:11434');
  const [connectionStatus, setConnectionStatus] = useState<ConnectionStatus>('idle');
  const [ollamaVersion, setOllamaVersion] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [models, setModels] = useState<OllamaModelInfo[]>([]);
  const [scanError, setScanError] = useState<string | null>(null);
  const [addingModel, setAddingModel] = useState<string | null>(null);

  const handleTestConnection = async () => {
    setConnectionStatus('testing');
    setOllamaVersion(null);
    try {
      const resp = await testOllamaConnection(endpoint);
      if (resp.success) {
        setConnectionStatus('connected');
        setOllamaVersion(resp.version ?? null);
        showToast('Ollama 连接成功', 'success');
      } else {
        setConnectionStatus('disconnected');
        showToast(resp.error || '连接失败', 'error');
      }
    } catch (err: unknown) {
      setConnectionStatus('disconnected');
      const msg = err instanceof Error ? err.message : '连接测试失败';
      showToast(msg, 'error');
    }
  };

  const handleScanModels = async () => {
    setScanning(true);
    setScanError(null);
    try {
      const resp = await scanOllamaModels(endpoint);
      if (resp.success) {
        setModels(resp.models);
        setConnectionStatus('connected');
        if (resp.models.length === 0) {
          showToast('未找到模型', 'info');
        } else {
          showToast(`找到 ${resp.models.length} 个模型`, 'success');
        }
      } else {
        setScanError(resp.error ?? '扫描失败');
        showToast(resp.error || '扫描失败', 'error');
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '扫描请求失败';
      setScanError(msg);
      showToast(msg, 'error');
    } finally {
      setScanning(false);
    }
  };

  const handleAddToConfig = async (model: OllamaModelInfo) => {
    setAddingModel(model.name);
    try {
      await apiPost('/config/ollama/add-model', {
        endpoint,
        model_name: model.name,
        capabilities: model.capabilities,
      });
      showToast(`${model.name} 已添加到配置`, 'success');
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '添加失败';
      showToast(`添加失败: ${msg}`, 'error');
    } finally {
      setAddingModel(null);
    }
  };

  const formatSize = (sizeStr: string) => {
    const num = parseFloat(sizeStr);
    if (isNaN(num)) return sizeStr;
    if (num >= 1e9) return `${(num / 1e9).toFixed(1)} GB`;
    if (num >= 1e6) return `${(num / 1e6).toFixed(1)} MB`;
    return sizeStr;
  };

  const connectionDot =
    connectionStatus === 'connected'
      ? 'bg-green-500'
      : connectionStatus === 'disconnected'
        ? 'bg-red-500'
        : 'bg-gray-500';

  const connectionLabel =
    connectionStatus === 'connected'
      ? `已连接${ollamaVersion ? ` (v${ollamaVersion})` : ''}`
      : connectionStatus === 'disconnected'
        ? '未连接'
        : connectionStatus === 'testing'
          ? '测试中...'
          : '未检测';

  return (
    <div>
      <SectionTitle title="Ollama 配置" />

      {/* Endpoint */}
      <FieldRow
        label="Ollama 服务地址"
        description="Ollama API 端点地址，默认 http://localhost:11434"
      >
        <div className="flex gap-2">
          <input
            type="text"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            className="flex-1 px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            placeholder="http://localhost:11434"
          />
          <button
            onClick={handleTestConnection}
            disabled={connectionStatus === 'testing'}
            className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors"
          >
            {connectionStatus === 'testing' ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <Wifi size={14} />
            )}
            测试连接
          </button>
        </div>
      </FieldRow>

      {/* Connection status */}
      <div className="flex items-center gap-2 mt-2 mb-4 px-1">
        <span className={`inline-block w-2.5 h-2.5 rounded-full ${connectionDot}`} />
        <span className="text-xs text-[var(--color-text-secondary)]">{connectionLabel}</span>
      </div>

      {/* Scan models */}
      <FieldRow label="扫描本地模型" description="扫描 Ollama 中已下载的模型并列出">
        <div className="flex gap-2">
          <button
            onClick={handleScanModels}
            disabled={scanning}
            className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 transition-colors"
          >
            {scanning ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
            {scanning ? '扫描中...' : '扫描模型'}
          </button>
        </div>
      </FieldRow>

      {/* Scan error */}
      {scanError && (
        <div className="mt-3 p-3 rounded-md bg-red-500/10 border border-red-500/30 text-sm text-red-400">
          {scanError}
        </div>
      )}

      {/* Model list */}
      {models.length > 0 && (
        <div className="mt-4">
          <h4 className="text-sm font-medium text-[var(--color-text-primary)] mb-2">
            已发现模型 ({models.length})
          </h4>
          <div className="space-y-2">
            {models.map((model) => (
              <div
                key={model.name}
                className="flex items-center justify-between p-3 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
              >
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-[var(--color-text-primary)] truncate">
                      {model.name}
                    </span>
                    {model.size && (
                      <span className="text-xs text-[var(--color-text-tertiary)] shrink-0">
                        {formatSize(model.size)}
                      </span>
                    )}
                  </div>
                  {model.capabilities && model.capabilities.length > 0 && (
                    <div className="flex flex-wrap gap-1 mt-1">
                      {model.capabilities.map((cap) => (
                        <span
                          key={cap}
                          className="px-1.5 py-0.5 text-[10px] rounded-md bg-accent/10 text-accent border border-accent/20"
                        >
                          {cap}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
                <button
                  onClick={() => handleAddToConfig(model)}
                  disabled={addingModel === model.name}
                  className="flex items-center gap-1 ml-3 px-2.5 py-1.5 text-xs rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors shrink-0"
                >
                  {addingModel === model.name ? (
                    <Loader2 size={12} className="animate-spin" />
                  ) : (
                    <Plus size={12} />
                  )}
                  添加到配置
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Empty state */}
      {!scanning && models.length === 0 && !scanError && (
        <div className="mt-4 p-6 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-center">
          <WifiOff size={24} className="mx-auto mb-2 text-[var(--color-text-tertiary)]" />
          <p className="text-sm text-[var(--color-text-tertiary)]">
            点击"扫描模型"查看已下载的 Ollama 模型
          </p>
        </div>
      )}
    </div>
  );
}
