import { useCallback, useEffect, useRef, useState } from 'react';
import { Toggle, FieldRow, NumberInput, SectionTitle, TextInput, INPUT_CLASS } from './shared';
import ConfirmDialog from '@/components/ui/ConfirmDialog';
import { getApiRoot } from '@/lib/api-base';
import { migrateDataDir } from '@/lib/api-client';
import { useAppStore } from '@/lib/store';
import { FolderOpen, Loader2, MoveRight } from 'lucide-react';
import type { ConfigState } from '@/lib/types';

interface StorageTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

/** 健康检查轮询间隔（搬迁后等待服务器重启）。 */
const HEALTH_POLL_INTERVAL_MS = 2000;
/** 等待服务器重启的最长时间。 */
const HEALTH_POLL_TIMEOUT_MS = 120000;

/** 数据目录搬迁区：选新目录 → 确认 → 后端关停 → 监督循环搬迁 → 自动重启。 */
function MigrationSection({ currentDir }: { currentDir: string }) {
  const showToast = useAppStore((s) => s.showToast);
  const [newDir, setNewDir] = useState('');
  const [confirmOpen, setConfirmOpen] = useState(false);
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
          if (pollTimer.current) clearInterval(pollTimer.current);
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
  }, [showToast]);

  const handleConfirm = useCallback(async () => {
    setConfirmOpen(false);
    setMigrating(true);
    setPhase('requested');
    try {
      await migrateDataDir(newDir.trim());
      // 请求已发出：服务器即将关停，连接会断开——进入健康检查轮询
      startHealthPoll();
    } catch (err: unknown) {
      setMigrating(false);
      setPhase('idle');
      const msg = err instanceof Error ? err.message : '未知错误';
      showToast(`搬迁启动失败: ${msg}`, 'error');
    }
  }, [newDir, showToast, startHealthPoll]);

  const canStart = newDir.trim().length > 0 && !migrating;

  return (
    <div className="mt-8 pt-6 border-t border-[var(--color-border)]">
      <h4 className="text-sm font-semibold text-[var(--color-text-primary)] flex items-center gap-1.5">
        <FolderOpen size={14} />
        数据目录搬迁
      </h4>
      <p className="text-xs text-[var(--color-text-tertiary)] mt-1.5">
        将全部数据（数据库 / 向量索引 / 快照 / 定时任务等）移动到新目录。
        程序会自动完成：关闭数据库 → 移动数据 → 更新配置 → 重启应用。
        当前目录：<span className="font-mono">{currentDir || '(默认)'}</span>
      </p>

      <div className="flex items-center gap-2 mt-3">
        <TextInput
          value={newDir}
          onChange={setNewDir}
          placeholder="新数据目录（绝对路径，如 D:\tianyan-data）"
          ariaLabel="新数据目录"
        />
        <button
          onClick={() => setConfirmOpen(true)}
          disabled={!canStart}
          className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed shrink-0"
        >
          {migrating ? <Loader2 size={12} className="animate-spin" /> : <MoveRight size={12} />}
          {migrating ? '搬迁中...' : '开始搬迁'}
        </button>
      </div>

      {phase === 'requested' && (
        <p className="text-xs text-[var(--color-text-secondary)] mt-2">
          搬迁请求已提交，正在关闭服务器...
        </p>
      )}
      {phase === 'waiting' && (
        <p className="text-xs text-[var(--color-text-secondary)] mt-2 flex items-center gap-1.5">
          <Loader2 size={12} className="animate-spin" />
          正在移动数据并重启应用，请稍候...
        </p>
      )}
      {phase === 'done' && (
        <p className="text-xs text-green-600 dark:text-green-400 mt-2">
          搬迁完成，应用已使用新数据目录运行
        </p>
      )}

      <ConfirmDialog
        open={confirmOpen}
        title="确认搬迁数据目录？"
        message={`将把全部数据从\n${currentDir || '(默认)'}\n移动到\n${newDir.trim()}\n\n搬迁期间应用会短暂关闭并自动重启。请确认目标目录为空或不存在。`}
        confirmLabel="确认搬迁"
        danger
        onConfirm={() => void handleConfirm()}
        onCancel={() => setConfirmOpen(false)}
      />
    </div>
  );
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

      <MigrationSection currentDir={config.data_dir} />
    </div>
  );
}
