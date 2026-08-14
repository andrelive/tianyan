import { Toggle, FieldRow, SectionTitle } from './shared';
import type { ConfigState } from '@/lib/types';

interface AgentTabProps {
  config: ConfigState;
  onUpdateField: <K extends keyof ConfigState>(key: K, value: ConfigState[K]) => void;
}

export default function AgentTab({ config, onUpdateField }: AgentTabProps) {
  return (
    <div>
      <SectionTitle title="Agent 行为" />
      <div className="space-y-4">
        <div className="grid grid-cols-2 gap-4">
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-32">启用技能</label>
            <Toggle
              checked={config.enable_skills}
              onChange={(v) => onUpdateField('enable_skills', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-32">启用记忆</label>
            <Toggle
              checked={config.enable_memory}
              onChange={(v) => onUpdateField('enable_memory', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-32">流式响应</label>
            <Toggle
              checked={config.stream_responses}
              onChange={(v) => onUpdateField('stream_responses', v)}
            />
          </div>
          <div className="flex items-center gap-3">
            <label className="text-sm text-[var(--color-text-primary)] w-32">启用思考</label>
            <Toggle
              checked={config.enable_thinking}
              onChange={(v) => onUpdateField('enable_thinking', v)}
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-4 pt-2">
          <FieldRow label="默认 Top-K">
            <input
              type="number"
              min={1}
              max={100}
              value={config.default_top_k}
              onChange={(e) => onUpdateField('default_top_k', parseInt(e.target.value) || 5)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </FieldRow>
          <FieldRow label="最大对话轮次">
            <input
              type="number"
              min={1}
              max={200}
              value={config.max_turns}
              onChange={(e) => onUpdateField('max_turns', parseInt(e.target.value) || 200)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </FieldRow>
          <FieldRow label="学习规则 Top-K">
            <input
              type="number"
              min={1}
              max={50}
              value={config.learned_rules_top_k}
              onChange={(e) => onUpdateField('learned_rules_top_k', parseInt(e.target.value) || 5)}
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </FieldRow>
          <FieldRow label="学习规则最大 Token">
            <input
              type="number"
              min={100}
              max={10000}
              value={config.learned_rules_max_tokens}
              onChange={(e) =>
                onUpdateField('learned_rules_max_tokens', parseInt(e.target.value) || 800)
              }
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </FieldRow>
        </div>

        {/* 工作目录：Agent 执行命令/读写文件的基础目录，会话回退快照的根目录 */}
        <div className="pt-2">
          <FieldRow label="工作目录">
            <input
              type="text"
              aria-label="工作目录"
              value={config.working_directory}
              onChange={(e) => onUpdateField('working_directory', e.target.value)}
              placeholder="留空 = 使用进程当前目录（如 E:\\code\\my-project）"
              className="w-full px-2.5 py-1.5 text-sm rounded-md border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </FieldRow>
          <p className="mt-1.5 text-xs text-[var(--color-text-tertiary)]">
            Agent 执行命令/读写文件的基础目录，也是会话回退时文件快照的根目录。
            文件视图需要配置后才能使用。
          </p>
        </div>
      </div>
    </div>
  );
}
