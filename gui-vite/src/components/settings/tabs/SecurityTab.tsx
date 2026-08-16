import { Toggle, FieldRow, SectionTitle } from './shared';
import type { ConfigState } from '@/lib/types';

interface SecurityTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function SecurityTab({ config, onUpdateField }: SecurityTabProps) {
  return (
    <div>
      <SectionTitle title="安全设置" />
      <div className="space-y-4">
        <div className="grid grid-cols-2 gap-4">
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-36">安全模式</label>
            <Toggle
              checked={config.security_enabled}
              onChange={(v) => onUpdateField('security_enabled', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-36">确认命令执行</label>
            <Toggle
              checked={config.confirm_commands}
              onChange={(v) => onUpdateField('confirm_commands', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-36">审计日志</label>
            <Toggle
              checked={config.audit_logging}
              onChange={(v) => onUpdateField('audit_logging', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-36">完全放开（不询问）</label>
            <Toggle
              checked={config.allow_all_operations}
              onChange={(v) => onUpdateField('allow_all_operations', v)}
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-4 pt-2">
          <FieldRow label="最大文件大小 (字节)">
            <input
              type="number"
              min={0}
              value={config.max_file_size}
              onChange={(e) => onUpdateField('max_file_size', parseInt(e.target.value) || 0)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </FieldRow>
        </div>

        <FieldRow label="允许的目录" description="每行一个目录路径">
          <textarea
            value={config.allowed_directories}
            onChange={(e) => onUpdateField('allowed_directories', e.target.value)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent resize-none"
            rows={3}
            placeholder="/home/user/projects"
          />
        </FieldRow>
        <FieldRow label="阻止的目录" description="每行一个目录路径">
          <textarea
            value={config.blocked_directories}
            onChange={(e) => onUpdateField('blocked_directories', e.target.value)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent resize-none"
            rows={3}
            placeholder="/etc"
          />
        </FieldRow>
        <FieldRow label="允许的命令" description="每行一个命令">
          <textarea
            value={config.allowed_commands}
            onChange={(e) => onUpdateField('allowed_commands', e.target.value)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent resize-none"
            rows={3}
            placeholder="ls, cat, git"
          />
        </FieldRow>
        <FieldRow label="阻止的命令" description="每行一个命令">
          <textarea
            value={config.blocked_commands}
            onChange={(e) => onUpdateField('blocked_commands', e.target.value)}
            className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent resize-none"
            rows={3}
            placeholder="rm, sudo, dd"
          />
        </FieldRow>

        <SectionTitle title="剪贴板（复制即记忆）" />
        <p className="text-xs text-[var(--color-text-tertiary)]">
          隐私敏感功能，默认关闭。开启后监听系统剪贴板，捕获内容经确认后沉淀到记忆/知识库（数据仅本机处理）。
        </p>
        <div className="grid grid-cols-2 gap-4 pt-2">
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-36">启用剪贴板监听</label>
            <Toggle
              checked={config.clipboard_enabled}
              onChange={(v) => onUpdateField('clipboard_enabled', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-36">自动沉淀（跳过确认）</label>
            <Toggle
              checked={config.clipboard_auto_capture}
              onChange={(v) => onUpdateField('clipboard_auto_capture', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-36">显示确认条</label>
            <Toggle
              checked={config.clipboard_prompt_confirm}
              onChange={(v) => onUpdateField('clipboard_prompt_confirm', v)}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
