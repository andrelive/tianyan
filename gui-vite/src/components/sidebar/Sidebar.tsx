import { useNavigate, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import {
  MessageSquare,
  Wrench,
  BookOpen,
  MemoryStick,
  Route,
  ShieldCheck,
  ListTodo,
  Gauge,
  Settings,
} from 'lucide-react';

/**
 * 全局一级导航侧边栏（图标 rail）：恒为 64px 图标栏，hover 显示名字（title）。
 * 不做展开/收起——一级菜单只保留图标更简洁；会话列表在会话页左栏（SessionList）。
 */
const NAV_ITEMS = [
  { id: 'chat' as const, label: '会话', icon: MessageSquare, path: '/chat' },
  { id: 'skills' as const, label: '技能', icon: Wrench, path: '/skills' },
  { id: 'knowledge' as const, label: '知识', icon: BookOpen, path: '/knowledge' },
  { id: 'memory' as const, label: '记忆', icon: MemoryStick, path: '/memory' },
  { id: 'traces' as const, label: '检索轨迹', icon: Route, path: '/traces' },
  { id: 'approval' as const, label: '审批', icon: ShieldCheck, path: '/approval' },
  { id: 'tasks' as const, label: '任务', icon: ListTodo, path: '/tasks' },
  { id: 'insights' as const, label: '洞察', icon: Gauge, path: '/insights' },
  { id: 'settings' as const, label: '设置', icon: Settings, path: '/settings' },
];

export default function Sidebar() {
  const navigate = useNavigate();
  const location = useLocation();
  const setView = useAppStore((s) => s.setView);

  const handleNavClick = (item: (typeof NAV_ITEMS)[number]) => {
    setView(item.id);
    navigate(item.path);
  };

  // Current path determines which nav is active
  const pathBase = location.pathname.split('/')[1] || 'chat';
  const activeNav = NAV_ITEMS.find((n) => n.id === pathBase)?.id || 'chat';

  return (
    <aside
      role="navigation"
      aria-label="导航"
      className="w-[64px] flex flex-col bg-[var(--color-bg-secondary)] border-r border-[var(--color-border)] shrink-0"
    >
      <div className="flex flex-col items-center gap-1 py-3">
        {/* 品牌 */}
        <div
          className="w-9 h-9 mb-1 flex items-center justify-center rounded-lg bg-blue-600/10 text-blue-600 dark:text-blue-400 font-bold text-sm select-none"
          title="天演"
        >
          天
        </div>
        {NAV_ITEMS.map((item) => (
          <button
            key={item.id}
            onClick={() => handleNavClick(item)}
            className={`p-2 rounded-md transition-colors ${
              activeNav === item.id
                ? 'text-blue-600 bg-blue-50 dark:bg-blue-900/30'
                : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
            }`}
            title={item.label}
            aria-label={item.label}
          >
            <item.icon size={20} />
          </button>
        ))}
      </div>
    </aside>
  );
}
