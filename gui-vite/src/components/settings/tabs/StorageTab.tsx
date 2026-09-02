import { useCallback, useEffect, useRef, useState } from 'react';
import { Toggle, FieldRow, NumberInput, SectionTitle, TextInput, INPUT_CLASS } from './shared';
import Modal from '@/components/ui/Modal';
import { getApiRoot } from '@/lib/api-base';
import { migrateDataDir } from '@/lib/api-client';
import { isTauri, pickDirectory } from '@/lib/tauri';
import { useAppStore } from '@/lib/store';
import { FolderInput, Loader2, MoveRight } from 'lucide-react';
import type { ConfigState } from '@/lib/types';

/** 健康检查轮询间隔（搬迁后等待服务器重启）。 */
const HEALTH_POLL_INTERVAL_MS = 2000;
/** 等待服务器重启的最长时间。 */
const HEALTH_POLL_TIMEOUT_MS = 120000;

/**
 * 数据目录搬迁对话框（认可交互：数据目录仅展示 + 搬迁按钮 → 对话框内
 * 选择新目录 → 确认；系统校验目标目录为空后才真正开始搬迁）。
 *
 * 空目录校验在后端 `validate_migration_target`（非空 → 400，错误信息
 * 就地展示，不触发关停）；校验通过后写迁移请求 → 服务器优雅关停 →
 * 监督循环搬迁 → 自动重启（本组件轮询 /health 等待重启完成）。
 */
function MigrationDialog({
  currentDir,
  onClose,
}: {
  currentDir: string;
  onClose: () => void;
}) {
  const showToast = useAppStore((s) => s.showToast);
  const [newDir, setNewDir] = useState('');
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [migrating, setMigrating] = useState(false);
  const [phase, setPhase] = useState<'idle' | 'requested' | 'waiting' | 'done'>('idle');
  const pollTimer = useRef<ReturnType<typeof setInterval> | null>(null);

  // 卸载时清理轮询
  useEffect(() => {
    return () => {
      if (pollTimer.current) clearInterval(pollTimer.current);
    };
  }, []);

  /** 搬迁请求发出后：轮询 /health 直到服务器重启完成。 */
  const startHealthPoll = useCallback(() => {
    setPhase('waiting');
    const root = getApiRoot();
    const startedAt = Date.now();
    pollTimer.current = setInterval(async () => {
      try {
        const resp = await fetch(`${root}/health`, { signal: AbortSignal.timeout(3000) });
        if (resp.ok) {
          // 搬迁是否真正生效：回滚时服务器以旧配置重启，data_dir 不会变成新目录。
          // 读取重启后的配置比对，避免把"回滚"误报成"搬迁完成"。
          let rolledBack = false;
          let gotDir = '';
          try {
            const cfgResp = await fetch(`${root}/api/v1/config`, { signal: AbortSignal.timeout(5000) });
            if (cfgResp.ok) {
              const body = (await cfgResp.json()) as { config?: { storage?: { data_dir?: string } } };
              const norm = (s: string) => s.trim().replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();
              gotDir = body?.config?.storage?.data_dir ?? '';
              rolledBack = gotDir.trim().length > 0 && norm(gotDir) !== norm(newDir.trim());
            }
          } catch {
            /* 配置读取失败：不阻塞，按成功流程处理 */
          }
          if (pollTimer.current) clearInterval(pollTimer.current);
          if (rolledBack) {
            setMigrating(false);
            setPhase('idle');
            setError(
              `搬迁未生效（已自动回滚）：当前数据目录仍为 ${gotDir || '(空)'}。请确认已完全退出应用（含托盘）后重试，详见应用日志。`,
            );
            showToast('数据目录搬迁未生效（已回滚）', 'error');
            return;
          }
          setPhase('done');
          setMigrating(false);
          showToast('数据目录搬迁完成', 'success');
          // 服务器已用新配置重启：刷新页面让配置/数据展示同步
          setTimeout(() => window.location.reload(), 1500);
        }
      } catch {
        /* 服务器未就绪，继续轮询 */
      }
      if (Date.now() - startedAt > HEALTH_POLL_TIMEOUT_MS) {
        if (pollTimer.current) clearInterval(pollTimer.current);
        setPhase('idle');
        setMigrating(false);
        showToast('等待服务器重启超时，请手动重启应用', 'error');
      }
    }, HEALTH_POLL_INTERVAL_MS);
  }, [newDir, showToast]);

  /** 原生目录选择（Tauri 桌面壳；浏览器环境回退为手动输入框）。 */
  const handlePick = useCallback(async () => {
    setError(null);
    setPicking(true);
    try {
      const dir = await pickDirectory('选择新数据目录（须为空目录）');
      if (dir) setNewDir(dir);
    } catch {
      setError('打开目录选择器失败，请手动输入路径');
    } finally {
      setPicking(false);
    }
  }, []);

  const handleConfirm = useCallback(async () => {
    setError(null);
    setMigrating(true);
    setPhase('requested');
    try {
      await migrateDataDir(newDir.trim());
      // 请求已发出：服务器即将关停，连接会断开——进入健康检查轮询
      startHealthPoll();
    } catch (err: unknown) {
      // 校验失败（400：目录非空/不可写等）不会触发关停，就地展示可重试
      setMigrating(false);
      setPhase('idle');
      setError(err instanceof Error ? err.message : '搬迁启动失败：未知错误');
    }
  }, [newDir, startHealthPoll]);

  const canConfirm = newDir.trim().length > 0 && !migrating;

  return (
    <Modal
      onClose={onClose}
      closeDisabled={migrating}
      ariaLabel="数据目录搬迁"
      panelClassName="w-full max-w-lg"
    >
      <div className="p-5 space-y-4">
        <div className="flex items-center gap-2">
          <FolderInput size={16} className="text-blue-600 dark:text-blue-400" />
          <h3 className="text-base font-semibold text-[var(--color-text-primary)]">数据目录搬迁</h3>
        </div>

        {phase === 'idle' && (
          <>
            <div className="space-y-1 text-sm">
              <p className="text-[var(--color-text-secondary)]">当前数据目录：</p>
              <p className="font-mono text-xs break-all rounded bg-[var(--color-bg-secondary)] border border-[var(--color-border)] px-2 py-1.5">
                {currentDir || '(默认)'}
              </p>
            </div>

            <div className="space-y-1.5">
              <p className="text-sm text-[var(--color-text-secondary)]">新数据目录（必须为空或不存在）：</p>
              {isTauri() ? (
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={() => void handlePick()}
                    disabled={picking}
                    className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
                  >
                    {picking ? <Loader2 size={12} className="animate-spin" /> : <FolderInput size={12} />}
                    {picking ? '打开选择器...' : '选择目录...'}
                  </button>
                  {newDir && (
                    <span className="min-w-0 flex-1 font-mono text-xs truncate" title={newDir}>
                      {newDir}
                    </span>
                  )}
                </div>
              ) : (
                <TextInput
                  value={newDir}
                  onChange={setNewDir}
                  placeholder="新数据目录（绝对路径，如 D:\tianyan-data）"
                  ariaLabel="新数据目录"
                />
              )}
              <p className="text-xs text-[var(--color-text-tertiary)]">
                确认后系统会先校验目标目录为空（避免其他文件影响），随后自动完成：
                关闭数据库 → 移动数据 → 更新配置 → 重启应用。搬迁期间应用会短暂关闭。
              </p>
            </div>

            {error && (
              <p className="text-xs text-red-600 dark:text-red-400 break-all" role="alert">
                {error}
              </p>
            )}

            <div className="flex justify-end gap-2 pt-1">
              <button
                type="button"
                onClick={onClose}
                disabled={migrating}
                className="px-3 py-1.5 text-xs rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50"
              >
                取消
              </button>
              <button
                type="button"
                onClick={() => void handleConfirm()}
                disabled={!canConfirm}
                className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {migrating ? <Loader2 size={12} className="animate-spin" /> : <MoveRight size={12} />}
                确认搬迁
              </button>
            </div>
          </>
        )}

        {phase === 'requested' && (
          <p className="text-xs text-[var(--color-text-secondary)] flex items-center gap-1.5">
            <Loader2 size={12} className="animate-spin" />
            搬迁请求已提交，正在关闭服务器...
          </p>
        )}
        {phase === 'waiting' && (
          <p className="text-xs text-[var(--color-text-secondary)] flex items-center gap-1.5">
            <Loader2 size={12} className="animate-spin" />
            正在校验并移动数据，完成后应用将自动重启，请稍候...
          </p>
        )}
        {phase === 'done' && (
          <p className="text-sm text-green-600 dark:text-green-400">
            搬迁完成，应用已使用新数据目录运行
          </p>
        )}
      </div>
    </Modal>
  );
}

interface StorageTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function StorageTab({ config, onUpdateField }: StorageTabProps) {
  const [migrateOpen, setMigrateOpen] = useState(false);

  return (
    <div>
      <SectionTitle title="数据存储" />
      <div className="grid grid-cols-2 gap-4">
        <FieldRow
          label="数据目录"
          description="数据库（tianyan.db）、向量索引、快照等所有本地数据的存储路径（通过「数据搬迁」更改）"
        >
          <div className="flex items-center gap-2">
            <code
              className="min-w-0 flex-1 px-2.5 py-1.5 text-sm rounded-md bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-primary)] font-mono truncate"
              title={config.data_dir}
            >
              {config.data_dir || '(默认)'}
            </code>
            <button
              type="button"
              onClick={() => setMigrateOpen(true)}
              aria-label="数据搬迁"
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md bg-accent text-white hover:bg-accent-hover shrink-0"
            >
              <FolderInput size={12} />
              数据搬迁
            </button>
          </div>
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
            <option value="512">512</option>
            <option value="768">768</option>
            <option value="1024">1024</option>
            <option value="1536">1536 (OpenAI ada-002 / text-embedding-3-small)</option>
            <option value="2048">2048 (阿里云 text-embedding-v4)</option>
            <option value="3072">3072 (text-embedding-3-large)</option>
            {![384, 512, 768, 1024, 1536, 2048, 3072].includes(config.vector_dimension) && (
              <option value={config.vector_dimension}>
                {config.vector_dimension}（当前值）
              </option>
            )}
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
                  onChange={(e) =>
                    onUpdateField('cleanup_days', parseInt(e.target.value) || 365)
                  }
                  className="w-20 px-2 py-1 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>
            )}
          </div>
        </div>
      </div>

      {migrateOpen && (
        <MigrationDialog currentDir={config.data_dir} onClose={() => setMigrateOpen(false)} />
      )}
    </div>
  );
}
