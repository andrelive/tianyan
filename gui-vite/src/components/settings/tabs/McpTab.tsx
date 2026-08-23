import { useState, useEffect, useCallback } from 'react';
import { Loader2, Plus, Trash2, WifiOff, Play, X, Check } from 'lucide-react';
import { useAppStore } from '@/lib/store';
import {
  listMcpServers,
  addMcpServer,
  removeMcpServer,
  toggleMcpServer,
  testMcpServer,
} from '@/lib/api-client';
import type { McpServerEntry } from '@/lib/types';
import { Toggle, FieldRow, SectionTitle } from './shared';
import ConfirmDialog from '@/components/ui/ConfirmDialog';

interface ServerTestStatus {
  name: string;
  status: 'idle' | 'testing' | 'success' | 'error';
  tools?: number;
  error?: string;
}

export default function McpTab() {
  const showToast = useAppStore((s) => s.showToast);

  const [servers, setServers] = useState<McpServerEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [testStatuses, setTestStatuses] = useState<Record<string, ServerTestStatus>>({});
  const [removing, setRemoving] = useState<string | null>(null);
/** 待移除确认的服务器（ConfirmDialog 状态机；替代 window.confirm） */
const [removeTarget, setRemoveTarget] = useState<string | null>(null);

  // Add form state
  const [formName, setFormName] = useState('');
  const [formCommand, setFormCommand] = useState('');
  const [formArgs, setFormArgs] = useState('');
  const [formDescription, setFormDescription] = useState('');
  const [formEnv, setFormEnv] = useState('');
  const [formSaving, setFormSaving] = useState(false);

  const fetchServers = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const data = await listMcpServers();
      setServers(data);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '无法加载 MCP 服务器列表';
      setLoadError(msg);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchServers();
  }, [fetchServers]);

  const handleToggle = async (server: McpServerEntry) => {
    try {
      await toggleMcpServer(server.name, !server.enabled);
      setServers((prev) =>
        prev.map((s) => (s.name === server.name ? { ...s, enabled: !s.enabled } : s)),
      );
      showToast(`${server.name} 已${server.enabled ? '禁用' : '启用'}`, 'success');
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '操作失败';
      showToast(msg, 'error');
    }
  };

  const handleTest = async (server: McpServerEntry) => {
    setTestStatuses((prev) => ({
      ...prev,
      [server.name]: { name: server.name, status: 'testing' },
    }));
    try {
      const resp = await testMcpServer(server.name);
      if (resp.success) {
        setTestStatuses((prev) => ({
          ...prev,
          [server.name]: {
            name: server.name,
            status: 'success',
            tools: resp.tools,
          },
        }));
        showToast(`${server.name} 连接成功 (${resp.tools} 个工具)`, 'success');
      } else {
        setTestStatuses((prev) => ({
          ...prev,
          [server.name]: {
            name: server.name,
            status: 'error',
            error: resp.error,
          },
        }));
        showToast(`${server.name} 连接失败: ${resp.error}`, 'error');
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '测试失败';
      setTestStatuses((prev) => ({
        ...prev,
        [server.name]: { name: server.name, status: 'error', error: msg },
      }));
      showToast(msg, 'error');
    }
  };

  const handleRemove = async (name: string) => {
    setRemoving(name);
    try {
      await removeMcpServer(name);
      setServers((prev) => prev.filter((s) => s.name !== name));
      showToast(`${name} 已移除`, 'success');
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '移除失败';
      showToast(`移除失败: ${msg}`, 'error');
    } finally {
      setRemoving(null);
    }
  };

  const handleAdd = async () => {
    if (!formName.trim() || !formCommand.trim()) {
      showToast('名称和命令为必填项', 'error');
      return;
    }
    setFormSaving(true);
    try {
      const args = formArgs
        .split(',')
        .map((a) => a.trim())
        .filter(Boolean);
      const env: Record<string, string> = {};
      if (formEnv.trim()) {
        formEnv.split(',').forEach((pair) => {
          const eqIdx = pair.indexOf('=');
          if (eqIdx > 0) {
            env[pair.slice(0, eqIdx).trim()] = pair.slice(eqIdx + 1).trim();
          }
        });
      }
      const newServer: McpServerEntry = {
        name: formName.trim(),
        command: formCommand.trim(),
        args,
        env: Object.keys(env).length > 0 ? env : undefined,
        enabled: true,
        description: formDescription.trim() || undefined,
      };
      await addMcpServer(newServer);
      setServers((prev) => [...prev, newServer]);
      setShowAddForm(false);
      setFormName('');
      setFormCommand('');
      setFormArgs('');
      setFormDescription('');
      setFormEnv('');
      showToast(`${newServer.name} 已添加`, 'success');
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '添加失败';
      showToast(`添加失败: ${msg}`, 'error');
    } finally {
      setFormSaving(false);
    }
  };

  const resetForm = () => {
    setShowAddForm(false);
    setFormName('');
    setFormCommand('');
    setFormArgs('');
    setFormEnv('');
    setFormDescription('');
  };

  /* ── Loading state ── */
  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 size={20} className="animate-spin text-[var(--color-text-tertiary)]" />
      </div>
    );
  }

  /* ── Error state ── */
  if (loadError) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 py-12">
        <X size={24} className="text-[var(--color-error)]" />
        <p className="text-sm text-[var(--color-error)]">{loadError}</p>
        <button
          onClick={fetchServers}
          className="px-3 py-1.5 text-sm rounded-md bg-accent text-white hover:bg-accent-hover transition-colors"
        >
          重试
        </button>
      </div>
    );
  }

  return (
    <div>
      <SectionTitle title="MCP 服务器" />

      {/* Server list */}
      {servers.length > 0 ? (
        <div className="space-y-2 mb-4">
          {servers.map((server) => {
            const testInfo = testStatuses[server.name];
            return (
              <div
                key={server.name}
                className="flex items-start gap-3 p-3 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)]"
              >
                {/* Toggle */}
                <div className="pt-0.5">
                  <Toggle checked={server.enabled} onChange={() => handleToggle(server)} />
                </div>

                {/* Info */}
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-[var(--color-text-primary)]">
                      {server.name}
                    </span>
                    {testInfo?.status === 'testing' && (
                      <Loader2
                        size={12}
                        className="animate-spin text-[var(--color-text-tertiary)]"
                      />
                    )}
                    {testInfo?.status === 'success' && (
                      <Check size={12} className="text-green-500" />
                    )}
                    {testInfo?.status === 'error' && <X size={12} className="text-red-500" />}
                  </div>
                  {server.description && (
                    <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
                      {server.description}
                    </p>
                  )}
                  <div className="flex items-center gap-1.5 mt-1">
                    <code className="text-[11px] px-1 py-0.5 rounded bg-[var(--color-bg-tertiary)] text-[var(--color-text-secondary)]">
                      {server.command}
                      {server.args.length > 0 && ` ${server.args.join(' ')}`}
                    </code>
                  </div>

                  {/* Test error */}
                  {testInfo?.status === 'error' && testInfo.error && (
                    <p className="text-xs text-red-400 mt-1">{testInfo.error}</p>
                  )}
                  {testInfo?.status === 'success' && testInfo.tools !== undefined && (
                    <p className="text-xs text-green-400 mt-1">{testInfo.tools} 个工具可用</p>
                  )}
                </div>

                {/* Actions */}
                <div className="flex items-center gap-1 shrink-0">
                  <button
                    onClick={() => handleTest(server)}
                    disabled={testInfo?.status === 'testing'}
                    className="p-1.5 rounded-md text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 transition-colors"
                    title="测试连接"
                  >
                    {testInfo?.status === 'testing' ? (
                      <Loader2 size={14} className="animate-spin" />
                    ) : (
                      <Play size={14} />
                    )}
                  </button>
                  <button
                    onClick={() => setRemoveTarget(server.name)}
                    disabled={removing === server.name}
                    className="p-1.5 rounded-md text-[var(--color-text-tertiary)] hover:text-red-400 hover:bg-red-500/10 disabled:opacity-50 transition-colors"
                    title="移除服务器"
                  >
                    {removing === server.name ? (
                      <Loader2 size={14} className="animate-spin" />
                    ) : (
                      <Trash2 size={14} />
                    )}
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      ) : (
        /* Empty state */
        <div className="p-6 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-center mb-4">
          <WifiOff size={24} className="mx-auto mb-2 text-[var(--color-text-tertiary)]" />
          <p className="text-sm text-[var(--color-text-tertiary)]">尚未配置 MCP 服务器</p>
          <p className="text-xs text-[var(--color-text-tertiary)] mt-1">
            MCP 服务器用于扩展 AI 助手的能力，提供文件操作、代码搜索等功能
          </p>
        </div>
      )}

      {/* Add server button */}
      {!showAddForm && (
        <button
          onClick={() => setShowAddForm(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md bg-accent text-white hover:bg-accent-hover transition-colors"
        >
          <Plus size={14} />
          添加服务器
        </button>
      )}

      {/* Add server form */}
      {showAddForm && (
        <div className="p-4 rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)] space-y-3">
          <h4 className="text-sm font-medium text-[var(--color-text-primary)]">添加 MCP 服务器</h4>

          <FieldRow label="名称">
            <input
              type="text"
              value={formName}
              onChange={(e) => setFormName(e.target.value)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="例如: filesystem-server"
            />
          </FieldRow>

          <FieldRow label="命令" description="启动命令，例如 npx、node、python 等">
            <input
              type="text"
              value={formCommand}
              onChange={(e) => setFormCommand(e.target.value)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="例如: npx"
            />
          </FieldRow>

          <FieldRow label="参数" description="命令参数，用逗号分隔">
            <input
              type="text"
              value={formArgs}
              onChange={(e) => setFormArgs(e.target.value)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="例如: @anthropic/mcp-serve, start"
            />
          </FieldRow>

          <FieldRow label="环境变量" description="可选，格式: KEY1=value1, KEY2=value2">
            <input
              type="text"
              value={formEnv}
              onChange={(e) => setFormEnv(e.target.value)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="例如: GITHUB_TOKEN=ghp_xxx, API_KEY=sk-xxx"
            />
          </FieldRow>

          <FieldRow label="描述" description="可选，简要说明此服务器的用途">
            <input
              type="text"
              value={formDescription}
              onChange={(e) => setFormDescription(e.target.value)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
              placeholder="文件系统操作、代码搜索等"
            />
          </FieldRow>

          <div className="flex items-center gap-2 pt-1">
            <button
              onClick={handleAdd}
              disabled={formSaving}
              className="flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors"
            >
              {formSaving ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
              保存
            </button>
            <button
              onClick={resetForm}
              disabled={formSaving}
              className="px-3 py-1.5 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 transition-colors"
            >
              取消
            </button>
          </div>
        </div>
      )}

      {/* 移除确认（统一 ConfirmDialog 原语） */}
      <ConfirmDialog
        open={removeTarget !== null}
        title="移除 MCP 服务器"
        message={removeTarget ? `确定要移除 "${removeTarget}" 吗？` : ''}
        danger
        confirmLabel="移除"
        busy={removing === removeTarget}
        onConfirm={() => {
          if (removeTarget) void handleRemove(removeTarget);
        }}
        onCancel={() => setRemoveTarget(null)}
      />
    </div>
  );
}
