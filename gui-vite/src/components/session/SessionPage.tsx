import SessionList from './SessionList';
import ChatPanel from '@/components/chat/ChatPanel';

/**
 * 会话页（点击一级导航「会话」后渲染）：左右分栏，
 * 左栏 = 会话二级列表（工作区分组，deepseek harness 式），
 * 右栏 = 聊天界面（消息/输入/模型/压缩/回退等）。
 */
export default function SessionPage() {
  return (
    <div className="flex h-full min-h-0">
      <SessionList />
      <main className="flex-1 min-w-0 flex flex-col bg-[var(--color-bg-primary)]">
        <ChatPanel />
      </main>
    </div>
  );
}
