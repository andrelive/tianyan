import { useState } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { usePolling } from '@/hooks/use-polling';
import { fetchApprovalStatus, fetchTasks } from '@/lib/api-client';
import {
  FileText,
  MessageSquare,
  Wrench,
  Users,
  Hammer,
  BookOpen,
  MemoryStick,
  Route,
  ShieldCheck,
  CalendarClock,
  Gauge,
  Settings,
} from 'lucide-react';

/**
 * 全局一级导航侧边栏（图标 rail）：恒为 64px 图标栏，hover 显示名字（title）。
 * 不做展开/收起——一级菜单只保留图标更简洁；会话列表在会话页左栏（SessionList）。
 * 审批/会话图标带动态角标（待审批数 / 全局运行中后台任务数），低频轮询（5s）。
 *
 * 任务语义分层（0.2 修复）：内置调度任务在「洞察」展示；定时智能体任务
 * 独立一级栏目；后台任务（委托/终端）会话绑定，在会话页内展示——不再设
 * 混合的「任务」栏目。待办/目标是智能体的会话内推理辅助工具，同样随会话
 * 展示，无全局「计划」栏目。
 */
const NAV_ITEMS = [
  { id: 'chat' as const, label: '会话', icon: MessageSquare, path: '/chat' },
  { id: 'workspace' as const, label: '文件', icon: FileText, path: '/workspace' },
  { id: 'skills' as const, label: '技能', icon: Wrench, path: '/skills' },
  { id: 'roles' as const, label: '子智能体', icon: Users, path: '/roles' },
  { id: 'tools' as const, label: '工具', icon: Hammer, path: '/tools' },
  { id: 'knowledge' as const, label: '知识', icon: BookOpen, path: '/knowledge' },
  { id: 'memory' as const, label: '记忆', icon: MemoryStick, path: '/memory' },
  { id: 'traces' as const, label: '检索轨迹', icon: Route, path: '/traces' },
  { id: 'approval' as const, label: '审批', icon: ShieldCheck, path: '/approval' },
  { id: 'scheduled' as const, label: '定时任务', icon: CalendarClock, path: '/scheduled' },
  { id: 'insights' as const, label: '洞察', icon: Gauge, path: '/insights' },
  { id: 'settings' as const, label: '设置', icon: Settings, path: '/settings' },
];

export default function Sidebar() {
  const navigate = useNavigate();
  const location = useLocation();
  const setView = useAppStore((s) => s.setView);
  const [pendingApprovals, setPendingApprovals] = useState(0);
  const [runningTasks, setRunningTasks] = useState(0);

  // 角标轮询（低频；失败静默保留旧值）
  usePolling(
    async () => {
      try {
        const status = await fetchApprovalStatus();
        setPendingApprovals(status.pending_approvals?.length ?? 0);
      } catch {
        /* 静默 */
      }
      try {
        const tasks = await fetchTasks();
        setRunningTasks(
          tasks.filter((t) => t.status === 'pending' || t.status === 'running').length,
        );
      } catch {
        /* 静默 */
      }
    },
    5000,
    { onError: () => {} },
  );

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
        {NAV_ITEMS.map((item) => {
          const badge =
            item.id === 'approval'
              ? pendingApprovals
              : item.id === 'chat'
                ? runningTasks
                : 0;
          return (
            <button
              key={item.id}
              onClick={() => handleNavClick(item)}
              className={`relative p-2 rounded-md transition-colors ${
                activeNav === item.id
                  ? 'text-blue-600 bg-blue-50 dark:bg-blue-900/30'
                  : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
              }`}
              title={item.label}
              aria-label={item.label}
            >
              <item.icon size={20} />
              {badge > 0 && (
                <span className="absolute top-0.5 right-0.5 min-w-[14px] h-[14px] px-0.5 flex items-center justify-center rounded-full bg-red-500 text-white text-[9px] font-bold leading-none">
                  {badge > 99 ? '99+' : badge}
                </span>
              )}
            </button>
          );
        })}
      </div>
    </aside>
  );
}
