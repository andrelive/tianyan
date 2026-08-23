import { useState, useEffect, useCallback } from 'react';
import { useResource } from '@/hooks/use-resource';
import { toErrorMessage } from '@/lib/errors';
import {
  Cpu,
  Database,
  Bot,
  Shield,
  FileText,
  Brain,
  Search,
  Palette,
  Wifi,
  Info,
  Save,
  Loader2,
  X,
  Server,
  User,
  PieChart,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { useAppStore } from '@/lib/store';
import { apiGet, apiPost, apiPut } from '@/lib/api-client';
import {
  fromBackendConfig,
  toBackendConfig,
  emptyProvider,
  emptyModelEntry,
} from '@/lib/config-transform';
import type {
  ConfigState,
  ProviderConfigState,
  ProviderModelEntry,
  ModelCapability,
  ModelPreferencesState,
  ModelRef,
  DiscoveredModelInfo,
} from '@/lib/types';
import type { BackendConfigResponse } from '@/lib/config-transform';

import ModelsTab from './tabs/ModelsTab';
import StorageTab from './tabs/StorageTab';
import AgentTab from './tabs/AgentTab';
import SecurityTab from './tabs/SecurityTab';
import LoggingTab from './tabs/LoggingTab';
import MemoryTab from './tabs/MemoryTab';
import RetrievalTab from './tabs/RetrievalTab';
import AppearanceTab from './tabs/AppearanceTab';
import ConnectionTab from './tabs/ConnectionTab';
import AboutTab from './tabs/AboutTab';
import McpTab from './tabs/McpTab';
import SoulTab from './tabs/SoulTab';
import UsageTab from './tabs/UsageTab';

/* ───────── Tab definitions ───────── */

interface TabDef {
  id: string;
  label: string;
  icon: LucideIcon;
}

const TABS: TabDef[] = [
  { id: 'models', label: '模型服务', icon: Cpu },
  { id: 'storage', label: '数据存储', icon: Database },
  { id: 'agent', label: 'Agent 行为', icon: Bot },
  { id: 'soul', label: '人设编辑', icon: User },
  { id: 'security', label: '安全', icon: Shield },
  { id: 'logging', label: '日志', icon: FileText },
  { id: 'memory', label: '记忆', icon: Brain },
  { id: 'retrieval', label: '检索', icon: Search },
  { id: 'appearance', label: '外观', icon: Palette },
  { id: 'connection', label: '连接', icon: Wifi },
  { id: 'mcp', label: 'MCP', icon: Server },
  { id: 'usage', label: '用量统计', icon: PieChart },
  { id: 'about', label: '关于', icon: Info },
];

/* ══════════ Inner panel (config guaranteed non-null) ══════════ */

function SettingsPanelContent({ config: cfg }: { config: ConfigState }) {
  const showToast = useAppStore((s) => s.showToast);

  const [activeTab, setActiveTab] = useState('models');
  const [config, setConfig] = useState<ConfigState>(cfg);
  const [saving, setSaving] = useState(false);

  /* Test connection state per provider (runtime only, not persisted) */
  const [testStatus, setTestStatus] = useState<
    Record<number, 'idle' | 'testing' | 'success' | 'error'>
  >({});

  /* Sync config when prop changes (e.g. after navigating back) */
  useEffect(() => {
    setConfig(cfg);
  }, [cfg]);

  /* Generic field updater */
  const updateField = useCallback(<K extends keyof ConfigState>(key: K, value: ConfigState[K]) => {
    setConfig((prev) => (prev ? { ...prev, [key]: value } : prev));
  }, []);

  /* ── Provider (model service) helpers ── */

  const addProvider = useCallback(() => {
    setConfig((prev) => {
      if (!prev) return prev;
      const p = emptyProvider();
      p.name = `provider-${prev.providers.length + 1}`;
      return { ...prev, providers: [...prev.providers, p] };
    });
  }, []);

  const removeProvider = useCallback((index: number) => {
    setConfig((prev) => {
      if (!prev) return prev;
      const providers = prev.providers.filter((_, i) => i !== index);
      return { ...prev, providers };
    });
  }, []);

  const updateProvider = useCallback(
    (index: number, field: keyof ProviderConfigState, value: unknown) => {
      setConfig((prev) => {
        if (!prev) return prev;
        const providers = prev.providers.map((p, i) =>
          i === index ? { ...p, [field]: value } : p,
        );
        return { ...prev, providers };
      });
    },
    [],
  );

  /* ── Model entry helpers ── */

  const addModel = useCallback((providerIndex: number) => {
    setConfig((prev) => {
      if (!prev) return prev;
      const providers = prev.providers.map((p, i) => {
        if (i !== providerIndex) return p;
        return { ...p, models: [...p.models, emptyModelEntry()] };
      });
      return { ...prev, providers };
    });
  }, []);

  const removeModel = useCallback((providerIndex: number, modelIndex: number) => {
    setConfig((prev) => {
      if (!prev) return prev;
      const providers = prev.providers.map((p, i) => {
        if (i !== providerIndex) return p;
        return { ...p, models: p.models.filter((_, mi) => mi !== modelIndex) };
      });
      return { ...prev, providers };
    });
  }, []);

  const updateModel = useCallback(
    (
      providerIndex: number,
      modelIndex: number,
      field: keyof ProviderModelEntry,
      value: unknown,
    ) => {
      setConfig((prev) => {
        if (!prev) return prev;
        const providers = prev.providers.map((p, i) => {
          if (i !== providerIndex) return p;
          const models = p.models.map((m, mi) =>
            mi === modelIndex ? { ...m, [field]: value } : m,
          );
          return { ...p, models };
        });
        return { ...prev, providers };
      });
    },
    [],
  );

  const toggleModelCapability = useCallback(
    (providerIndex: number, modelIndex: number, cap: ModelCapability) => {
      setConfig((prev) => {
        if (!prev) return prev;
        const providers = prev.providers.map((p, i) => {
          if (i !== providerIndex) return p;
          const models = p.models.map((m, mi) => {
            if (mi !== modelIndex) return m;
            const has = m.capabilities.includes(cap);
            return {
              ...m,
              capabilities: has
                ? m.capabilities.filter((c) => c !== cap)
                : [...m.capabilities, cap],
            };
          });
          return { ...p, models };
        });
        return { ...prev, providers };
      });
    },
    [],
  );

  /* Add scanned models (from provider discovery) into the provider's model list,
     carrying endpoint/catalog-provided spec fields (context/max output/efforts) */
  const addScannedModels = useCallback((providerIndex: number, scanned: DiscoveredModelInfo[]) => {
    if (scanned.length === 0) return;
    setConfig((prev) => {
      if (!prev) return prev;
      const providers = prev.providers.map((p, i) =>
        i === providerIndex
          ? {
              ...p,
              models: [
                ...p.models,
                ...scanned.map((m) => ({
                  name: m.name,
                  capabilities: m.capabilities as ModelCapability[],
                  context_length: m.context_length,
                  max_output_tokens: m.max_output_tokens,
                  reasoning_efforts:
                    m.reasoning_efforts && m.reasoning_efforts.length > 0
                      ? m.reasoning_efforts
                      : undefined,
                })),
              ],
            }
          : p,
      );
      return { ...prev, providers };
    });
  }, []);

  /* ── Preferences helpers ── */

  type PreferenceKey = keyof ModelPreferencesState;

  const updatePreference = useCallback((key: PreferenceKey, provider: string, model: string) => {
    setConfig((prev) => {
      if (!prev) return prev;
      const ref: ModelRef | null = provider && model ? { provider, model } : null;
      return {
        ...prev,
        preferences: { ...prev.preferences, [key]: ref },
      };
    });
  }, []);

  /* Test model connection — uses backend test-connection endpoint */
  const testConnection = useCallback(
    async (index: number) => {
      const p = config.providers[index];
      if (!p) return;
      if (!p.endpoint || !p.api_key) {
        setTestStatus((prev) => ({ ...prev, [index]: 'error' }));
        showToast('请填写 Endpoint 和 API Key', 'error');
        return;
      }
      // Use the first model in the provider for testing, or a generic name
      const modelName = p.models[0]?.name || 'test-model';
      setTestStatus((prev) => ({ ...prev, [index]: 'testing' }));
      try {
        const resp = await apiPost<{ success: boolean; message: string }>(
          '/config/test-connection',
          {
            endpoint: p.endpoint,
            api_key: p.api_key,
            model: modelName,
          },
        );
        if (resp.success) {
          setTestStatus((prev) => ({ ...prev, [index]: 'success' }));
          showToast(`${p.name} 连接成功`, 'success');
        } else {
          setTestStatus((prev) => ({ ...prev, [index]: 'error' }));
          showToast(`${p.name} 连接失败: ${resp.message}`, 'error');
        }
      } catch (err: unknown) {
        setTestStatus((prev) => ({ ...prev, [index]: 'error' }));
        const msg = toErrorMessage(err, '未知错误');
        showToast(`${p.name} 连接失败: ${msg}`, 'error');
      }
    },
    [config.providers, showToast],
  );

  /* Save config */
  const handleSave = useCallback(async () => {
    setSaving(true);
    try {
      const payload = toBackendConfig(config);
      await apiPut<{ success: boolean; message: string }>('/config', payload);
      showToast('设置已保存', 'success');
      // 保存成功后重新拉取配置：刷新后端解析结果（model_specs → resolvedSpecs，
      // 即"生效规格"行）。否则生效规格停留在面板挂载时的旧快照
      // （如手动设置 spec 后仍显示内置表匹配的旧值）。
      const fresh = await apiGet<BackendConfigResponse>('/config');
      setConfig(fromBackendConfig(fresh));
    } catch (err: unknown) {
      const msg = toErrorMessage(err, '保存失败');
      showToast(`保存失败: ${msg}`, 'error');
    } finally {
      setSaving(false);
    }
  }, [config, showToast]);

  /* ── Whether to show Save button ── */
  const showSave =
    activeTab !== 'appearance' &&
    activeTab !== 'connection' &&
    activeTab !== 'soul' &&
    activeTab !== 'mcp' &&
    activeTab !== 'usage' &&
    activeTab !== 'about';

  /* ── Render active tab ── */
  function renderActiveTab() {
    switch (activeTab) {
      case 'models':
        return (
          <ModelsTab
            config={config}
            onAddProvider={addProvider}
            onRemoveProvider={removeProvider}
            onUpdateProvider={updateProvider}
            onAddModel={addModel}
            onRemoveModel={removeModel}
            onUpdateModel={updateModel}
            onToggleModelCapability={toggleModelCapability}
            onUpdatePreference={updatePreference}
            onTestConnection={testConnection}
            testStatus={testStatus}
            onAddScannedModels={addScannedModels}
          />
        );
      case 'storage':
        return <StorageTab config={config} onUpdateField={updateField} />;
      case 'agent':
        return <AgentTab config={config} onUpdateField={updateField} />;
      case 'soul':
        return <SoulTab />;
      case 'security':
        return <SecurityTab config={config} onUpdateField={updateField} />;
      case 'logging':
        return <LoggingTab config={config} onUpdateField={updateField} />;
      case 'memory':
        return <MemoryTab config={config} onUpdateField={updateField} />;
      case 'retrieval':
        return <RetrievalTab config={config} onUpdateField={updateField} />;
      case 'appearance':
        return <AppearanceTab />;
      case 'connection':
        return <ConnectionTab />;
      case 'mcp':
        return <McpTab />;
      case 'usage':
        return <UsageTab />;
      case 'about':
        return <AboutTab />;
      default:
        return null;
    }
  }

  /* ── Main render ── */

  return (
    // min-h-0：flex-col 子项默认 min-height:auto，高内容（模型服务页）时
    // 会把容器撑开并向 document 泄漏滚动（整个设置界面随 body 滚动），
    // 导致滚到底后左栏 tab 导航等被滚出视野
    <div className="flex-1 flex overflow-hidden min-h-0">
      {/* Left: tab navigation */}
      <nav
        role="tablist"
        aria-label="设置选项卡"
        className="w-44 shrink-0 border-r border-[var(--color-border)] bg-[var(--color-bg-secondary)] overflow-y-auto"
      >
        {TABS.map((tab) => {
          const Icon = tab.icon;
          return (
            <button
              key={tab.id}
              role="tab"
              id={`tab-${tab.id}`}
              aria-selected={activeTab === tab.id}
              aria-controls={`tabpanel-${tab.id}`}
              onClick={() => setActiveTab(tab.id)}
              className={`w-full flex items-center gap-2.5 px-4 py-2.5 text-sm text-left transition-colors ${
                activeTab === tab.id
                  ? 'bg-[var(--color-bg-primary)] text-accent font-medium border-l-2 border-accent'
                  : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] border-l-2 border-transparent'
              }`}
            >
              <Icon size={16} />
              <span>{tab.label}</span>
            </button>
          );
        })}
      </nav>

      {/* Right: content */}
      <div
        className="flex-1 overflow-y-auto"
        role="tabpanel"
        id={`tabpanel-${activeTab}`}
        aria-labelledby={`tab-${activeTab}`}
      >
        <div className="max-w-2xl mx-auto p-6">
          {renderActiveTab()}

          {showSave && (
            <div className="mt-8 pt-4 border-t border-[var(--color-border)] flex justify-end">
              <button
                onClick={handleSave}
                disabled={saving}
                className="flex items-center gap-2 px-5 py-2 text-sm font-medium rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors"
              >
                {saving ? <Loader2 size={16} className="animate-spin" /> : <Save size={16} />}
                {saving ? '保存中...' : '保存设置'}
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/* ══════════ Outer component (loading / error / content) ══════════ */

export default function SettingsPanel() {
  // 配置加载（useResource：加载/错误/reload 收敛；fetcher 内做后端 → 表单转换）
  const {
    data: config,
    loading,
    error: loadError,
    reload,
  } = useResource(
    async () => fromBackendConfig(await apiGet<BackendConfigResponse>('/config')),
    [],
    { errorFallback: '无法加载配置' },
  );

  if (loading) {
    return (
      <div
        className="flex-1 flex items-center justify-center min-h-0"
        aria-live="polite"
        aria-label="正在加载设置"
      >
        <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
      </div>
    );
  }

  if (loadError || !config) {
    return (
      <div
        className="flex-1 flex flex-col items-center justify-center gap-3 p-8 min-h-0"
        role="alert"
      >
        <X size={32} className="text-[var(--color-error)]" />
        <p className="text-[var(--color-error)] text-sm">{loadError || '无法加载配置'}</p>
        <button
          onClick={reload}
          className="px-4 py-1.5 text-sm rounded-md bg-accent text-white hover:bg-accent-hover transition-colors"
        >
          重新加载
        </button>
      </div>
    );
  }

  return <SettingsPanelContent config={config} />;
}
