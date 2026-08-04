import { Bot, Cpu, Database } from 'lucide-react';
import type { StepProps } from './wizard.types';

export default function WelcomeStep(_props: StepProps) {
  return (
    <div className="flex flex-col items-center text-center py-8">
      <Bot size={64} className="text-accent mb-4" />
      <h1 className="text-2xl font-bold text-[var(--color-text-primary)] mb-2">欢迎使用天演</h1>
      <p className="text-sm text-[var(--color-text-secondary)] max-w-md mb-8 leading-relaxed">
        天演是一个基于大语言模型的本地智能代理系统。 在开始之前，我们需要完成一些基本配置。
        整个过程大约需要 2 分钟。
      </p>

      <div className="grid grid-cols-3 gap-4 w-full max-w-lg mb-8">
        {[
          { icon: Cpu, title: '配置模型', desc: '添加 AI 服务提供商' },
          { icon: Database, title: '数据存储', desc: '设置存储路径' },
          { icon: Bot, title: 'Agent 行为', desc: '自定义交互方式' },
        ].map((item) => {
          const Icon = item.icon;
          return (
            <div
              key={item.title}
              className="flex flex-col items-center gap-2 p-3 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-secondary)]"
            >
              <Icon size={24} className="text-accent" />
              <span className="text-sm font-medium text-[var(--color-text-primary)]">
                {item.title}
              </span>
              <span className="text-xs text-[var(--color-text-tertiary)]">{item.desc}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
