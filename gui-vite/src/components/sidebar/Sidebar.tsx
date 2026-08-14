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
  Plus,
  PanelLeftClose,
  PanelLeft,
} from 'lucide-react';

/**
 * 全局一级导航侧边栏：纯导航，不含会话列表（会话列表在会话页左栏，见 SessionList）。
 */
const NAV_ITEMS = [
  { id: 'chat' as const, label: '会话', icon: MessageSquare, path: '/chat' },
  { id: 'skills' as const, label: '技能', icon: Wrench, path: '/skills' },
  { id: 'knowledge' as const, label: '知识', icon: BookOpen, path: '/knowledge' },
  {
    id: 'memory' as const,
    label: '记忆',
    icon: MemoryStick,
    path: '/memory',
  },
  {
    id: 'traces' as const,
    label: '检索轨迹',
    icon: Route,
    path: '/traces',
  },
  {
    id: 'approval' as const,
    label: '审批',
    icon: ShieldCheck,
    path: '/approval',
  },
  {
    id: 'tasks' as const,
    label: '任务',
    icon: ListTodo,
    path: '/tasks',
  },
  {
    id: 'insights' as const,
    label: '洞察',
    icon: Gauge,
    path: '/insights',
  },
  {
    id: 'settings' as const,
    label: '设置',
    icon: Settings,
    path: '/settings',
  },
];

export default function Sidebar() {
  const navigate = useNavigate();
  const location = useLocation();
  const isSidebarOpen = useAppStore((s) => s.isSidebarOpen);
  const toggleSidebar = useAppStore((s) => s.toggleSidebar);
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const setMessages = useAppStore((s) => s.setMessages);
  const setView = useAppStore((s) => s.setView);
  const setNewSessionWorkspace = useAppStore((s) => s.setNewSessionWorkspace);

  /** 新建会话：进入会话页空状态（默认组，零弹窗；会话页左栏可再选目录）。 */
  const handleNewChat = () => {
    setNewSessionWorkspace(null);
    setCurrentSession(null);
    setMessages([]);
    setView('chat');
    navigate('/chat');
  };

  const handleNavClick = (item: (typeof NAV_ITEMS)[number]) => {
    setView(item.id);
    navigate(item.path);
  };

  // Current path determines which nav is active
  const pathBase = location.pathname.split('/')[1] || 'chat';
  const activeNav = NAV_ITEMS.find((n) => n.id === pathBase)?.id || 'chat';

  if (!isSidebarOpen) {
    return (
      <aside
        role="navigation"
        aria-label="导航"
        className="w-[64px] flex flex-col bg-[var(--color-bg-secondary)] border-r border-[var(--color-border)] shrink-0"
      >
        <div className="flex flex-col items-center gap-1 py-3">
          <button
            onClick={toggleSidebar}
            className="p-2 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)]"
            title="展开侧边栏"
            aria-label="展开侧边栏"
          >
            <PanelLeft size={20} />
          </button>
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

  return (
    <aside
      role="navigation"
      aria-label="导航"
      className="w-[280px] flex flex-col bg-[var(--color-bg-secondary)] border-r border-[var(--color-border)] shrink-0"
    >
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
        <h1 className="text-base font-semibold text-[var(--color-text-primary)]">天演</h1>
        <div className="flex items-center gap-1">
          <button
            onClick={handleNewChat}
            className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)]"
            title="新建会话"
            aria-label="新建会话"
          >
            <Plus size={18} />
          </button>
          <button
            onClick={toggleSidebar}
            className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)]"
            title="收起侧边栏"
            aria-label="收起侧边栏"
          >
            <PanelLeftClose size={18} />
          </button>
        </div>
      </div>

      {/* Nav items */}
      <nav className="flex flex-col px-2 py-2 gap-0.5">
        {NAV_ITEMS.map((item) => (
          <button
            key={item.id}
            onClick={() => handleNavClick(item)}
            className={`flex items-center gap-2.5 px-3 py-2 rounded-md text-sm transition-colors ${
              activeNav === item.id
                ? 'bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300 font-medium'
                : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
            }`}
          >
            <item.icon size={18} />
            <span>{item.label}</span>
          </button>
        ))}
      </nav>
    </aside>
  );
}
