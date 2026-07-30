import { Bot } from 'lucide-react';
import { SectionTitle } from './shared';

export default function AboutTab() {
  return (
    <div>
      <SectionTitle title="关于天演" />
      <div className="space-y-4 text-sm text-[var(--color-text-secondary)]">
        <div className="flex items-center gap-3 p-4 rounded-lg bg-[var(--color-bg-secondary)]">
          <Bot size={40} className="text-accent" />
          <div>
            <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">天演 Tianyan</h2>
            <p className="text-xs text-[var(--color-text-tertiary)]">版本 0.1.0</p>
          </div>
        </div>

        <p>
          天演是一个基于大语言模型的本地智能代理系统，通过自然语言交互帮助用户完成各种任务。
          它以大语言模型 (LLM) 为核心智能引擎，实现了统一的上下文管理架构。
        </p>

        <div>
          <h4 className="font-medium text-[var(--color-text-primary)] mb-1">技术栈</h4>
          <ul className="list-disc list-inside space-y-0.5 text-xs">
            <li>前端: React + TypeScript + Tailwind CSS</li>
            <li>后端: Rust + Axum</li>
            <li>桌面: Tauri</li>
            <li>向量数据库: LanceDB（嵌入式）</li>
            <li>状态管理: Zustand</li>
          </ul>
        </div>

        <div>
          <h4 className="font-medium text-[var(--color-text-primary)] mb-1">核心特性</h4>
          <ul className="list-disc list-inside space-y-0.5 text-xs">
            <li>统一上下文管理 (VFS 三层摘要索引)</li>
            <li>OpenAI API 标准兼容</li>
            <li>本地优先，数据隐私安全</li>
            <li>可扩展技能系统</li>
            <li>记忆自迭代与学习</li>
          </ul>
        </div>

        <p className="text-xs text-[var(--color-text-tertiary)]">基于 MIT 许可证开源发布</p>
      </div>
    </div>
  );
}
