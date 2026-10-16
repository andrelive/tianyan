/**
 * 打开会话（侧栏列表 / 最近会话条等入口共用）：切换 + 导航 + 历史加载。
 *
 * 懒加载收口（loadSessionHistory 唯一入口）：本地已有缓存直接显示；
 * 无缓存才拉历史。失败提示 toast，不阻塞切换。
 */

import { useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { loadSessionHistory } from '@/hooks/use-session-history';

export function useOpenSession(): (sessionId: string) => void {
  const navigate = useNavigate();
  return useCallback(
    (sessionId: string) => {
      const st = useAppStore.getState();
      if (st.currentSessionId === sessionId) return;
      st.setCurrentSession(sessionId);
      st.setView('chat');
      navigate(`/chat/${sessionId}`);
      void loadSessionHistory(sessionId).then((ok) => {
        if (!ok) useAppStore.getState().showToast('加载会话消息失败', 'error');
      });
    },
    [navigate],
  );
}
