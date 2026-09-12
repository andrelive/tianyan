import { lazy, Suspense, useEffect } from 'react';
import { Routes, Route, Navigate, useNavigate, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { apiGet } from '@/lib/api-client';
import type { ConfigStatus } from '@/lib/types';
import { useTheme } from '@/hooks/use-theme';
import { useKeyboardShortcuts } from '@/hooks/use-keyboard-shortcuts';
import AppLayout from '@/components/layout/AppLayout';
import ErrorBoundary from '@/components/layout/ErrorBoundary';
import Toast from '@/components/layout/Toast';

// 路由级代码分割（C6）：面板按需加载——首屏只带 chat 域代码，
// workspace/编辑器/设置等大块（prism/codemirror）进入对应 chunk
const SessionPage = lazy(() => import('@/components/session/SessionPage'));
const SkillsPanel = lazy(() => import('@/components/skills/SkillsPanel'));
const RolesPanel = lazy(() => import('@/components/roles/RolesPanel'));
const ToolsPanel = lazy(() => import('@/components/tools/ToolsPanel'));
const KnowledgePanel = lazy(() => import('@/components/knowledge/KnowledgePanel'));
const WorkspacePanel = lazy(() => import('@/components/workspace/WorkspacePanel'));
const MemoryPanel = lazy(() => import('@/components/memory/MemoryPanel'));
const ApprovalPanel = lazy(() => import('@/components/approval/ApprovalPanel'));
const ScheduledTasksPanel = lazy(() => import('@/components/tasks/ScheduledTasksPanel'));
const InsightsPanel = lazy(() => import('@/components/insights/InsightsPanel'));
const SettingsPanel = lazy(() => import('@/components/settings/SettingsPanel'));
const ConfigWizard = lazy(() => import('@/components/wizard/ConfigWizard'));

/** 路由级错误边界：每个面板独立捕获，单面板崩溃不拖垮整应用（含侧边栏）；
 * 导航时按 pathname 重置，切走再回来自动恢复。 */
function RouteErrorBoundary({ children }: { children: React.ReactNode }) {
  const { pathname } = useLocation();
  return <ErrorBoundary key={pathname}>{children}</ErrorBoundary>;
}

function App() {
  const theme = useAppStore((s) => s.theme);
  const fontSize = useAppStore((s) => s.fontSize);
  const configured = useAppStore((s) => s.configured);
  const setConfigured = useAppStore((s) => s.setConfigured);
  const setView = useAppStore((s) => s.setView);
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const clearMessages = useAppStore((s) => s.clearMessages);
  const navigate = useNavigate();

  useTheme(theme);

  // Font size（三档整体升档：小=14px、中=16px、大=18px——
  // 小取原中档、中取原大档、大按比例（16/14≈1.143）再高一级；
  // 对话历史字号类（text-lg/text-base）保持不变，随根字号等比缩放）
  useEffect(() => {
    document.documentElement.style.fontSize =
      fontSize === 'small' ? '14px' : fontSize === 'large' ? '18px' : '16px';
  }, [fontSize]);

  // Check config status on mount
  useEffect(() => {
    apiGet<ConfigStatus>('/config/status')
      .then((status) => setConfigured(status.configured))
      .catch(() => setConfigured(false));
  }, [setConfigured]);

  useKeyboardShortcuts({
    onNewChat: () => {
      setCurrentSession(null);
      clearMessages();
      setView('chat');
      navigate('/chat');
    },
    onClearMessages: () => clearMessages(),
    onOpenSettings: () => {
      setView('settings');
      navigate('/settings');
    },
  });

  if (configured === null) {
    return (
      <div className="h-screen flex items-center justify-center bg-[var(--color-bg-primary)]">
        <div className="flex flex-col items-center gap-3">
          <div className="w-8 h-8 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
          <p className="text-[var(--color-text-secondary)]">正在加载...</p>
        </div>
      </div>
    );
  }

  if (!configured) {
    return <ConfigWizard />;
  }

  return (
    <ErrorBoundary>
      <Toast />
      <Suspense
        fallback={
          <div className="h-full flex items-center justify-center bg-[var(--color-bg-primary)]">
            <div className="flex flex-col items-center gap-3">
              <div className="w-8 h-8 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
              <p className="text-[var(--color-text-secondary)]">加载中...</p>
            </div>
          </div>
        }
      >
        <Routes>
          <Route element={<AppLayout />}>
            <Route path="/" element={<Navigate to="/chat" replace />} />
            <Route path="/chat" element={<SessionPage />} />
            <Route path="/chat/:sessionId" element={<SessionPage />} />
            <Route
              path="/skills"
              element={
                <RouteErrorBoundary>
                  <SkillsPanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/roles"
              element={
                <RouteErrorBoundary>
                  <RolesPanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/tools"
              element={
                <RouteErrorBoundary>
                  <ToolsPanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/knowledge"
              element={
                <RouteErrorBoundary>
                  <KnowledgePanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/workspace"
              element={
                <RouteErrorBoundary>
                  <WorkspacePanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/memory"
              element={
                <RouteErrorBoundary>
                  <MemoryPanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/approval"
              element={
                <RouteErrorBoundary>
                  <ApprovalPanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/scheduled"
              element={
                <RouteErrorBoundary>
                  <ScheduledTasksPanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/insights"
              element={
                <RouteErrorBoundary>
                  <InsightsPanel />
                </RouteErrorBoundary>
              }
            />
            <Route
              path="/settings"
              element={
                <RouteErrorBoundary>
                  <SettingsPanel />
                </RouteErrorBoundary>
              }
            />
          </Route>
        </Routes>
      </Suspense>
    </ErrorBoundary>
  );
}

export default App;
