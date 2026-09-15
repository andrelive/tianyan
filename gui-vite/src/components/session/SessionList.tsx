import { useEffect, useState, useRef, useCallback, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { useResource } from '@/hooks/use-resource';
import { useAppStore } from '@/lib/store';
import {
  deleteSession,
  fetchSessionMessages,
  listSessions,
  updateSessionTitle,
} from '@/lib/api-client';
import type { Session } from '@/lib/types';
import { formatRelativeTime } from '@/lib/utils';
import {
  ChevronDown,
  ChevronRight,
  FileText,
  FolderOpen,
  FolderPlus,
  MessageSquare,
  Plus,
  Trash2,
} from 'lucide-react';
import WorkspacePicker from '@/components/workspace/WorkspacePicker';
import ConfirmDialog from '@/components/ui/ConfirmDialog';

/** 目录绝对路径 → 展示名（basename；默认组回退）。 */
function groupLabel(workdir: string): string {
  if (!workdir) return '默认';
  const parts = workdir.split(/[\\/]/).filter(Boolean);
  const base = parts.pop();
  return base || workdir;
}

/**
 * 会话页左栏（二级列表，对齐 deepseek harness 布局）：
 * 工作区分组（目录名）+ 会话子项；组行 hover「＋」在该目录新建会话（零弹窗），
 * 顶部「＋ 新建会话」（默认组）、「＋ 新目录」（目录选择器，唯一弹窗），
 * 底部「文件视图」入口（当前会话工作区审计页）。
 */
export default function SessionList() {
  const navigate = useNavigate();
  const sessions = useAppStore((s) => s.sessions);
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const clearMessages = useAppStore((s) => s.clearMessages);
  const setMessages = useAppStore((s) => s.setMessages);
  const setView = useAppStore((s) => s.setView);
  const showToast = useAppStore((s) => s.showToast);
  const newSessionWorkspace = useAppStore((s) => s.newSessionWorkspace);
  const setNewSessionWorkspace = useAppStore((s) => s.setNewSessionWorkspace);

  const [hoveredSession, setHoveredSession] = useState<string | null>(null);
  const [hoveredGroup, setHoveredGroup] = useState<string | null>(null);
  const [editingSessionId, setEditingSessionId] = useState<string | null>(null);
  const [editingTitle, setEditingTitle] = useState('');
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());
  const [pickerOpen, setPickerOpen] = useState(false);
  /** 用户是否已发起新会话（点击「＋ 新建会话」/分组「＋」/「新目录」后为 true）。 */
  const [newChatStarted, setNewChatStarted] = useState(false);
  const sessionListRef = useRef<HTMLDivElement>(null);
  const editInputRef = useRef<HTMLInputElement>(null);

  // 会话列表加载：隐藏在本组件内（useResource 收敛加载/竞态）；
  // 数据写入 store（sessions 分组渲染的真相源）
  const { reload: reloadSessions } = useResource(async () => {
    const data = await listSessions();
    useAppStore.getState().setSessions(data.sessions);
    return data;
  }, []);

  // 新会话创建完成（currentSessionId 从 null → 非 null）后触发列表刷新：
  // 让刚创建的真实会话条目出现在对应分组下。
  const prevSessionId = useRef<string | null>(null);
  useEffect(() => {
    if (currentSessionId && currentSessionId !== prevSessionId.current) {
      // 刷新会话列表（reload 不返回 Promise，不能 await；占位清理由
      // 下方“数据就绪” effect 驱动，不依赖刷新时序）
      reloadSessions();
    }
    prevSessionId.current = currentSessionId;
  }, [currentSessionId, reloadSessions]);

  // 占位与待绑定目录的清理由**数据就绪**驱动（U9）：新会话真实条目出现在
  // `sessions` 后才撤占位/清目录——此前在 currentSessionId 变化时立即撤，
  // 而列表刷新是异步的（reload 不返回 Promise，无法 await），中间窗口里
  // 新建工作目录的分组会先消失再重现（用户可见闪烁）。ChatPanel 也不再在
  // 会话创建瞬间清待绑定目录（清理统一收敛到这里；失败路径也不清，重试
  // 不丢目录）。
  useEffect(() => {
    if (newChatStarted && currentSessionId && sessions.some((s) => s.id === currentSessionId)) {
      setNewChatStarted(false);
      setNewSessionWorkspace(null);
    }
  }, [newChatStarted, currentSessionId, sessions, setNewSessionWorkspace]);

  // 进入编辑模式时聚焦输入框
  useEffect(() => {
    if (editingSessionId) {
      editInputRef.current?.focus();
      editInputRef.current?.select();
    }
  }, [editingSessionId]);

  // 按工作目录分组（工作区 = 会话的父级分组；未绑定归入默认组）。
  // 默认组（''）始终保留：即便没有未绑定会话，默认工作区也可见（可新建会话）。
  const sessionGroups = useMemo(() => {
    const map = new Map<string, Session[]>();
    for (const s of sessions) {
      const key = s.working_directory || '';
      const list = map.get(key) ?? [];
      list.push(s);
      map.set(key, list);
    }
    if (!map.has('')) {
      map.set('', []);
    }
    return [...map.entries()].sort((a, b) => {
      if (a[0] === '') return 1;
      if (b[0] === '') return -1;
      return a[0].localeCompare(b[0]);
    });
  }, [sessions]);
  const flatSessions = useMemo(() => sessionGroups.flatMap(([, list]) => list), [sessionGroups]);

  // 进行中的新会话（用户已发起、尚未持久化）：分组归属由待绑定目录决定。
  // 左栏在对应分组下显示「新会话」占位条目，用户可感知新会话属于哪个目录。
  // 仅当用户点击过「新建会话」后才显示占位（初始空列表仍显示「暂无会话」）。
  const pendingGroupKey = newChatStarted ? (newSessionWorkspace ?? '') : null;
  // 待绑定目录的分组必须可见：不在现有分组中时合成临时分组（含占位条目）。
  // 这样「添加新工作区」后左栏立即出现新分组，用户可见新会话归属。
  const groupsToRender: [string, Session[]][] = (() => {
    if (pendingGroupKey === null) return sessionGroups;
    const has = sessionGroups.some(([k]) => k === pendingGroupKey);
    if (has) return sessionGroups;
    const merged: [string, Session[]][] = [...sessionGroups];
    merged.push([pendingGroupKey, []]);
    merged.sort((a, b) => {
      if (a[0] === '') return 1;
      if (b[0] === '') return -1;
      return a[0].localeCompare(b[0]);
    });
    return merged;
  })();

  /** 开启新会话：绑定到指定目录（空串 = 默认组），零弹窗。 */
  const startNewSession = useCallback(
    (workdir: string) => {
      setNewSessionWorkspace(workdir || null);
      setCurrentSession(null);
      clearMessages();
      setNewChatStarted(true);
      setView('chat');
      navigate('/chat');
    },
    [navigate, setCurrentSession, clearMessages, setNewSessionWorkspace, setView],
  );

  /** 添加新目录分组：目录选择器（唯一弹窗）→ 进入该目录的新会话。 */
  const handleAddDirectory = useCallback(
    (path: string) => {
      setPickerOpen(false);
      startNewSession(path);
    },
    [startNewSession],
  );

  const handleSelectSession = useCallback(
    async (session: Session) => {
      setCurrentSession(session.id);
      setView('chat');
      navigate(`/chat/${session.id}`);
      // 本地已有缓存（流式累积/之前看过）→ 直接显示；无缓存才拉历史
      if (useAppStore.getState().hasSessionMessages(session.id)) return;
      try {
        const data = await fetchSessionMessages(session.id);
        setMessages(data.messages);
      } catch {
        showToast('加载会话消息失败', 'error');
      }
    },
    [navigate, setCurrentSession, setMessages, setView, showToast],
  );

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
      const current = useAppStore.getState().sessions;
      useAppStore
        .getState()
        .setSessions(current.map((s) => (s.id === sessionId ? { ...s, title } : s)));
    } catch {
      showToast('重命名会话失败', 'error');
    }
  };

  // 删除会话走统一 ConfirmDialog 原语（对齐 RolesPanel/KnowledgeBrowseTab），
  // 避免一次性不可逆操作无确认。
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; title: string } | null>(null);
  const handleDeleteClick = (e: React.MouseEvent, session: Session) => {
    e.stopPropagation();
    setDeleteTarget({ id: session.id, title: session.title || '新对话' });
  };
  const handleConfirmDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteSession(deleteTarget.id);
      useAppStore.getState().removeSession(deleteTarget.id);
      // 删除的是当前会话：跳回会话首页，避免停留在失效的 /chat/:deletedId
      if (currentSessionId === deleteTarget.id) navigate('/chat');
    } catch {
      showToast('删除会话失败', 'error');
    } finally {
      setDeleteTarget(null);
    }
  };

  const toggleGroup = (key: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const items = sessionListRef.current?.querySelectorAll('[role="button"]');
      if (!items || items.length === 0) return;
      // 以当前焦点元素在 DOM 列表中的位置为锚（分组行/占位/会话行混排，
      // 不能用与 flatSessions 错位的 index——否则 Enter 会选错会话）
      const focused = document.activeElement;
      const idx = Math.max(Array.from(items).indexOf(focused as HTMLElement), 0);
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        const next = Math.min(idx + 1, items.length - 1);
        (items[next] as HTMLElement).focus();
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        const prev = Math.max(idx - 1, 0);
        (items[prev] as HTMLElement).focus();
      } else if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        // 仅会话行带 data-session-id；分组/占位行无 → 自然 no-op
        const sid = (items[idx] as HTMLElement).dataset.sessionId;
        if (sid) {
          const session = flatSessions.find((s) => s.id === sid);
          if (session) handleSelectSession(session);
        }
      }
    },
    [flatSessions, handleSelectSession],
  );

  return (
    <aside
      aria-label="会话列表"
      className="w-[272px] shrink-0 flex flex-col border-r border-[var(--color-border)] bg-[var(--color-bg-secondary)] min-h-0"
    >
      {/* 头部：新建会话（默认组）/ 新目录 */}
      <div className="flex items-center gap-1 px-3 py-2 border-b border-[var(--color-border)]">
        <span className="text-xs font-medium text-[var(--color-text-secondary)]">会话</span>
        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            onClick={() => startNewSession('')}
            title="新建会话"
            aria-label="新建会话"
            className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)]"
          >
            <Plus size={16} />
          </button>
          <button
            type="button"
            onClick={() => setPickerOpen(true)}
            title="添加新工作区"
            aria-label="新目录"
            className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)]"
          >
            <FolderPlus size={16} />
          </button>
        </div>
      </div>

      {/* 会话列表（工作区分组 + 会话子项） */}
      {/* 默认组始终存在（sessionGroups 恒含 ''），故不再有「暂无会话」空态：
          无任何会话时仍显示默认工作区分组，可 hover「＋」新建会话 */}
      <div ref={sessionListRef} className="flex-1 overflow-y-auto px-2 py-2">
        <div className="flex flex-col gap-1.5" role="list" aria-label="会话列表">
          {groupsToRender.map(([workdir, groupSessions]) => {
            const collapsed = collapsedGroups.has(workdir);
            const label = groupLabel(workdir);
            return (
              <div key={workdir || '__default_ws__'} className="flex flex-col gap-0.5">
                {/* 分组行：折叠/展开 + hover「＋」新建该目录会话 */}
                <div
                  role="button"
                  tabIndex={0}
                  onClick={() => toggleGroup(workdir)}
                  onMouseEnter={() => setHoveredGroup(workdir)}
                  onMouseLeave={() => setHoveredGroup(null)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      toggleGroup(workdir);
                    }
                  }}
                  aria-label={`分组 ${label}`}
                  className="group flex items-center gap-1 px-1.5 py-1 rounded-md cursor-pointer hover:bg-[var(--color-bg-hover)]"
                >
                  {collapsed ? (
                    <ChevronRight
                      size={12}
                      className="shrink-0 text-[var(--color-text-tertiary)]"
                    />
                  ) : (
                    <ChevronDown size={12} className="shrink-0 text-[var(--color-text-tertiary)]" />
                  )}
                  <FolderOpen size={13} className="shrink-0 text-[var(--color-text-tertiary)]" />
                  <span
                    className="text-xs font-medium text-[var(--color-text-secondary)] truncate"
                    title={workdir || undefined}
                  >
                    {label}
                  </span>
                  <span className="text-[10px] text-[var(--color-text-tertiary)] shrink-0">
                    {groupSessions.length}
                  </span>
                  {hoveredGroup === workdir && (
                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        startNewSession(workdir);
                      }}
                      title={`在 ${label} 新建会话`}
                      aria-label={`在 ${label} 新建会话`}
                      className="ml-auto p-0.5 rounded hover:bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] shrink-0"
                    >
                      <Plus size={12} />
                    </button>
                  )}
                </div>

                {/* 进行中的新会话占位条目（归属当前分组） */}
                {!collapsed && pendingGroupKey === workdir && (
                  <div
                    role="button"
                    aria-label="新会话"
                    title="新会话（发送首条消息后创建）"
                    className="flex items-center gap-2 pl-7 pr-2 py-1.5 rounded-md text-sm border border-dashed border-blue-400/50 bg-blue-50/40 dark:bg-blue-900/10 text-blue-700 dark:text-blue-300"
                  >
                    <MessageSquare size={13} className="shrink-0" />
                    <span className="truncate">新会话</span>
                  </div>
                )}

                {/* 会话子项（二级） */}
                {!collapsed &&
                  groupSessions.map((session) => {
                    return (
                      <div
                        key={session.id}
                        role="button"
                        tabIndex={0}
                        data-session-id={session.id}
                        onClick={() => handleSelectSession(session)}
                        onKeyDown={handleKeyDown}
                        onMouseEnter={() => setHoveredSession(session.id)}
                        onMouseLeave={() => setHoveredSession(null)}
                        aria-label={session.title || '新对话'}
                        aria-current={currentSessionId === session.id ? 'true' : undefined}
                        className={`group flex items-center justify-between pl-7 pr-2 py-1.5 rounded-md cursor-pointer text-sm transition-colors ${
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
                            onClick={(e) => handleDeleteClick(e, session)}
                            className="p-1 rounded hover:bg-red-100 dark:hover:bg-red-900/30 text-[var(--color-text-tertiary)] hover:text-red-500 shrink-0"
                            title="删除会话"
                            aria-label={`删除会话 ${session.title || '新对话'}`}
                          >
                            <Trash2 size={14} />
                          </button>
                        )}
                      </div>
                    );
                  })}
              </div>
            );
          })}
        </div>
      </div>

      {/* 底部：文件视图入口（当前会话工作区审计页） */}
      <div className="border-t border-[var(--color-border)] px-2 py-2">
        <button
          type="button"
          onClick={() => navigate('/workspace')}
          className="w-full flex items-center gap-2 px-2 py-1.5 rounded-md text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors"
          title="打开当前会话的文件视图（树/diff 审计）"
          aria-label="文件视图"
        >
          <FileText size={14} />
          文件视图
        </button>
      </div>

      {/* 新目录：目录选择器（唯一弹窗） */}
      <WorkspacePicker
        open={pickerOpen}
        currentWorkingDir={newSessionWorkspace ?? ''}
        onClose={() => setPickerOpen(false)}
        onSelect={handleAddDirectory}
      />
      <ConfirmDialog
        open={!!deleteTarget}
        title="删除会话"
        message={'确定删除会话「' + (deleteTarget?.title ?? '新对话') + '」吗？此操作不可恢复。'}
        confirmLabel="删除"
        danger
        onConfirm={handleConfirmDelete}
        onCancel={() => setDeleteTarget(null)}
      />
    </aside>
  );
}
