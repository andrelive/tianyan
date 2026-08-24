import { FieldRow, NumberInput, SectionTitle, TextInput } from './shared';
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
        <p className="text-xs text-[var(--color-text-tertiary)]">
          技能执行、记忆持久化与流式响应是智能体的固有能力，始终开启；
          思考模式在会话输入区按对话选择（仅对支持思考的模型生效）。
        </p>

        <div className="grid grid-cols-2 gap-4 pt-2">
          <FieldRow label="默认 Top-K">
            <NumberInput
              value={config.default_top_k}
              onChange={(v) => onUpdateField('default_top_k', v)}
              min={1}
              max={100}
              fallback={5}
            />
          </FieldRow>
          <FieldRow label="最大对话轮次">
            <NumberInput
              value={config.max_turns}
              onChange={(v) => onUpdateField('max_turns', v)}
              min={1}
              max={200}
              fallback={200}
            />
          </FieldRow>
          <FieldRow label="学习规则 Top-K">
            <NumberInput
              value={config.learned_rules_top_k}
              onChange={(v) => onUpdateField('learned_rules_top_k', v)}
              min={1}
              max={50}
              fallback={5}
            />
          </FieldRow>
        </div>

        {/* 工作目录：Agent 执行命令/读写文件的基础目录，会话回退快照的根目录 */}
        <div className="pt-2">
          <FieldRow label="工作目录">
            <TextInput
              value={config.working_directory}
              onChange={(v) => onUpdateField('working_directory', v)}
              placeholder="留空 = 使用进程当前目录（如 E:\\code\\my-project）"
              ariaLabel="工作目录"
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
