import { useEffect, useState, useRef, useCallback } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { apiGet, apiDelete, updateSessionTitle } from '@/lib/api-client';
import type { Session, ListSessionsResponse, SessionMessagesResponse } from '@/lib/types';
import { formatRelativeTime } from '@/lib/utils';
import {
  MessageSquare,
  Wrench,
  BookOpen,
  FolderOpen,
  MemoryStick,
  Route,
  ShieldCheck,
  Gauge,
  Settings,
  Plus,
  PanelLeftClose,
  PanelLeft,
  Trash2,
} from 'lucide-react';

const NAV_ITEMS = [
  { id: 'chat' as const, label: '对话', icon: MessageSquare, path: '/chat' },
  { id: 'skills' as const, label: '技能', icon: Wrench, path: '/skills' },
  {
    id: 'knowledge' as const,
    label: '知识',
    icon: BookOpen,
    path: '/knowledge',
  },
  {
    id: 'workspace' as const,
    label: '工作区',
    icon: FolderOpen,
    path: '/workspace',
  },
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
  const sessions = useAppStore((s) => s.sessions);
  const setSessions = useAppStore((s) => s.setSessions);
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const setMessages = useAppStore((s) => s.setMessages);
  const showToast = useAppStore((s) => s.showToast);
  const setView = useAppStore((s) => s.setView);
  const [hoveredSession, setHoveredSession] = useState<string | null>(null);
  const [editingSessionId, setEditingSessionId] = useState<string | null>(null);
  const [editingTitle, setEditingTitle] = useState('');
  const sessionListRef = useRef<HTMLDivElement>(null);
  const editInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    apiGet<ListSessionsResponse>('/sessions')
      .then((data) => setSessions(data.sessions))
      .catch(() => {
        /* sessions optional for now */
      });
  }, [setSessions]);

  // 进入编辑模式时聚焦输入框
  useEffect(() => {
    if (editingSessionId) {
      editInputRef.current?.focus();
      editInputRef.current?.select();
    }
  }, [editingSessionId]);

  const handleStartRename = (e: React.MouseEvent, session: Session) => {
    e.stopPropagation();
    setEditingSessionId(session.id);
    setEditingTitle(session.title || '');
  };

  const handleRenameSubmit = async () => {
    if (!editingSessionId) return;
    const sessionId = editingSessionId;
    const title = editingTitle.trim();
    setEditingSessionId(null);
    if (!title) return;

    try {
      await updateSessionTitle(sessionId, title);
      // 更新本地会话列表中的标题
      const current = useAppStore.getState().sessions;
      useAppStore
        .getState()
        .setSessions(current.map((s) => (s.id === sessionId ? { ...s, title } : s)));
    } catch {
      showToast('重命名会话失败', 'error');
    }
  };

  const handleNewChat = () => {
    setCurrentSession(null);
    setMessages([]);
    setView('chat');
    navigate('/chat');
  };

  const handleSelectSession = async (session: Session) => {
    setCurrentSession(session.id);
    setView('chat');
    navigate(`/chat/${session.id}`);
    // Fetch messages for the selected session
    try {
      const data = await apiGet<SessionMessagesResponse>(`/sessions/${session.id}/messages`);
      setMessages(data.messages);
    } catch {
      showToast('加载会话消息失败', 'error');
    }
  };

  const handleDeleteSession = async (e: React.MouseEvent, id: string) => {
    e.stopPropagation();
    try {
      await apiDelete(`/sessions/${id}`);
      useAppStore.getState().removeSession(id);
    } catch {
      showToast('删除会话失败', 'error');
    }
  };

  const handleNavClick = (item: (typeof NAV_ITEMS)[number]) => {
    setView(item.id);
    navigate(item.path);
  };

  const handleSessionKeyDown = useCallback(
    (e: React.KeyboardEvent, index: number) => {
      const items = sessionListRef.current?.querySelectorAll('[role="button"]');
      if (!items) return;

      if (e.key === 'ArrowDown') {
        e.preventDefault();
        const next = Math.min(index + 1, items.length - 1);
        (items[next] as HTMLElement).focus();
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        const prev = Math.max(index - 1, 0);
        (items[prev] as HTMLElement).focus();
      } else if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        handleSelectSession(sessions[index]);
      }
    },
    [sessions, handleSelectSession],
  );

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
            title="新建对话"
            aria-label="新建对话"
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
      <nav className="flex flex-col px-2 py-2 gap-0.5 border-b border-[var(--color-border)]">
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

      {/* Session list */}
      <div className="flex-1 overflow-y-auto px-2 py-2">
        {sessions.length === 0 ? (
          <p
            className="text-xs text-[var(--color-text-tertiary)] text-center mt-8"
            aria-live="polite"
          >
            暂无会话
          </p>
        ) : (
          <div
            ref={sessionListRef}
            className="flex flex-col gap-0.5"
            role="list"
            aria-label="会话列表"
          >
            {sessions.map((session, i) => (
              <div
                key={session.id}
                role="button"
                tabIndex={0}
                onClick={() => handleSelectSession(session)}
                onKeyDown={(e) => handleSessionKeyDown(e, i)}
                onMouseEnter={() => setHoveredSession(session.id)}
                onMouseLeave={() => setHoveredSession(null)}
                aria-label={session.title || '新对话'}
                aria-current={currentSessionId === session.id ? 'true' : undefined}
                className={`group flex items-center justify-between px-3 py-2 rounded-md cursor-pointer text-sm transition-colors ${
                  currentSessionId === session.id
                    ? 'bg-blue-50 dark:bg-blue-900/20'
                    : 'hover:bg-[var(--color-bg-hover)]'
                }`}
              >
                <div className="flex-1 min-w-0">
                  {editingSessionId === session.id ? (
                    <input
                      ref={editInputRef}
                      value={editingTitle}
                      onChange={(e) => setEditingTitle(e.target.value)}
                      onBlur={handleRenameSubmit}
                      onKeyDown={(e) => {
                        e.stopPropagation();
                        if (e.key === 'Enter') {
                          handleRenameSubmit();
                        } else if (e.key === 'Escape') {
                          setEditingSessionId(null);
                        }
                      }}
                      className="w-full px-1 py-0.5 text-sm rounded border border-blue-500 bg-[var(--color-bg-primary)] text-[var(--color-text-primary)] outline-none"
                      aria-label="编辑会话标题"
                    />
                  ) : (
                    <p
                      onDoubleClick={(e) => handleStartRename(e, session)}
                      title="双击重命名"
                      className={`truncate ${
                        currentSessionId === session.id
                          ? 'text-blue-700 dark:text-blue-300 font-medium'
                          : 'text-[var(--color-text-primary)]'
                      }`}
                    >
                      {session.title || '新对话'}
                    </p>
                  )}
                  <p className="text-xs text-[var(--color-text-tertiary)] mt-0.5">
                    {formatRelativeTime(session.updated_at)}
                  </p>
                </div>
                {hoveredSession === session.id && (
                  <button
                    onClick={(e) => handleDeleteSession(e, session.id)}
                    className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500 shrink-0"
                    title="删除会话"
                    aria-label={`删除会话 ${session.title || '新对话'}`}
                  >
                    <Trash2 size={14} />
                  </button>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}
