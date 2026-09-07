import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { StrictMode, type ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '@/test/mocks/server';
import { useAppStore } from '@/lib/store';
import { useSessionHistory } from '../use-session-history';
import { messageText } from '@/lib/types';
import { applySnapshot, subscribeSession, __resetSubscriptions } from '../use-unified-events';

const API_BASE = '/api/v1';

function historyMessages() {
  return [
    {
      id: 'msg-1',
      role: 'user' as const,
      content: '你好',
      segments: [{ type: 'text' as const, text: '你好' }],
      timestamp: '',
    },
    {
      id: 'msg-2',
      role: 'assistant' as const,
      content: '你好！我是天演',
      segments: [{ type: 'text' as const, text: '你好！我是天演' }],
      timestamp: '',
    },
  ];
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  __resetSubscriptions();
});

describe('useSessionHistory', () => {
  it('subscribes on mount for the URL session; snapshot frame replaces the window', async () => {
    // 模拟 ChatPanel 的 URL 同步（/chat/{id} → setCurrentSession）
    useAppStore.getState().setCurrentSession('session-1');
    renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await Promise.resolve();
    });

    // 挂载触发订阅（POST /events/subscribe，MSW mock 返回 ok）；
    // 快照帧经统一事件通道到达（测试环境无 EventSource，直接驱动模拟）
    act(() => {
      applySnapshot('session-1', historyMessages());
    });

    const msgs = useAppStore.getState().messages;
    expect(msgs).toHaveLength(2);
    expect(msgs[0] ? messageText(msgs[0]) : undefined).toBe('你好');
    expect(msgs[1]?.role).toBe('assistant');
  });

  it('skips re-subscribe when already subscribed (resident)', async () => {
    // 已订阅（resident）：再次挂载不重复请求
    await subscribeSession('session-1');
    const subscribeCalls = vi.fn();
    server.use(
      http.post(`${API_BASE}/events/subscribe`, () => {
        subscribeCalls();
        return HttpResponse.json({ status: 'ok' });
      }),
    );

    renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await Promise.resolve();
    });

    expect(subscribeCalls).not.toHaveBeenCalled();
  });

  it('snapshot for a non-current session does not pollute the current projection', async () => {
    useAppStore.getState().setCurrentSession('session-2');
    renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await Promise.resolve();
    });

    act(() => {
      applySnapshot('session-1', historyMessages());
    });

    // 非当前会话：只写字典，不污染投影
    expect(useAppStore.getState().messages).toHaveLength(0);
    expect(useAppStore.getState().sessionMessages['session-1']).toHaveLength(2);
  });

  it('reloadSession overwrites the target session cache and current projection', async () => {
    useAppStore.getState().setCurrentSession('session-1');
    const { result } = renderHook(() => useSessionHistory('session-1'));

    await act(async () => {
      await result.current.reloadSession('session-1');
    });

    const msgs = useAppStore.getState().sessionMessages['session-1'] ?? [];
    expect(msgs).toHaveLength(2);
    expect(useAppStore.getState().messages).toHaveLength(2);
  });

  it('reloadSession falls back to an empty list on failure', async () => {
    server.use(
      http.get(
        `${API_BASE}/sessions/session-1/messages`,
        () => new HttpResponse(null, { status: 500 }),
      ),
    );
    useAppStore
      .getState()
      .setSessionMessages('session-1', [{ role: 'user', segments: [{ type: 'text', text: 'old' }], timestamp: '' }]);

    const { result } = renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await result.current.reloadSession('session-1');
    });

    expect(useAppStore.getState().sessionMessages['session-1']).toEqual([]);
  });

  it('subscribes only once under StrictMode double-mount (ref guard)', async () => {
    const subscribeCalls = vi.fn();
    server.use(
      http.post(`${API_BASE}/events/subscribe`, () => {
        subscribeCalls();
        return HttpResponse.json({ status: 'ok' });
      }),
    );
    const wrapper = ({ children }: { children: ReactNode }) => <StrictMode>{children}</StrictMode>;

    renderHook(() => useSessionHistory('session-1'), { wrapper });
    await act(async () => {
      await Promise.resolve();
    });

    expect(subscribeCalls).toHaveBeenCalledTimes(1);
  });
});


