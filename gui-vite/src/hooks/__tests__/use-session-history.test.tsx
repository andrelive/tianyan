import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { StrictMode, type ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '@/test/mocks/server';
import { useAppStore } from '@/lib/store';
import { loadSessionHistory, useSessionHistory } from '../use-session-history';
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

  it('skips re-subscribe when already subscribed and history loaded (resident)', async () => {
    // 已订阅（resident）+ 历史已加载（有分页元数据）：再次挂载不重复请求
    // （历史未加载时强制重发，见下一条用例）
    await subscribeSession('session-1');
    useAppStore.getState().setSessionMessageMeta('session-1', { oldestSeq: 12, hasMore: true });
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

  it('subscribeSession force bypasses the resident idempotence (history reload)', async () => {
    // 流式事件写过缓存但历史从未加载（无 meta）→ 必须能强制重发快照
    // （resident 幂等会把历史永久挡住）
    const subscribeCalls = vi.fn();
    server.use(
      http.post(`${API_BASE}/events/subscribe`, () => {
        subscribeCalls();
        return HttpResponse.json({ status: 'ok' });
      }),
    );

    await subscribeSession('session-1');
    await subscribeSession('session-1'); // resident 幂等：不发
    expect(subscribeCalls).toHaveBeenCalledTimes(1);

    await subscribeSession('session-1', { force: true }); // 强制重发
    expect(subscribeCalls).toHaveBeenCalledTimes(2);
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

  it('loadSessionHistory fetches only the latest page (limit=50) and records paging meta', async () => {
    let capturedUrl = '';
    server.use(
      http.get(`${API_BASE}/sessions/session-1/messages`, ({ request }) => {
        capturedUrl = request.url;
        return HttpResponse.json({
          session_id: 'session-1',
          messages: historyMessages(),
          next_before_seq: 12,
          has_more: true,
          last_seq: 20,
        });
      }),
    );

    const ok = await loadSessionHistory('session-1');

    expect(ok).toBe(true);
    expect(capturedUrl).toContain('limit=50');
    expect(capturedUrl).not.toContain('before_seq');
    const st = useAppStore.getState();
    expect(st.sessionMessages['session-1']).toHaveLength(2);
    expect(st.sessionMessageMeta['session-1']).toEqual({ oldestSeq: 12, hasMore: true });
  });

  it('fetches and merges when cache is non-empty but paging meta is missing', async () => {
    // 模拟流式事件残留（唤醒轮/后台轮按 session_id 写入，但不写分页元数据）：
    // 缓存里只有本轮 1 条 system 通知，历史从未加载
    useAppStore.setState({
      sessionMessages: {
        'session-1': [
          {
            id: 'live-1',
            role: 'system',
            segments: [{ type: 'text' as const, text: '[后台命令完成]' }],
            timestamp: '',
          },
        ],
      },
      sessionMessageMeta: {},
    });
    let calls = 0;
    server.use(
      http.get(`${API_BASE}/sessions/session-1/messages`, () => {
        calls += 1;
        return HttpResponse.json({
          session_id: 'session-1',
          messages: historyMessages(),
          next_before_seq: 12,
          has_more: true,
          last_seq: 20,
        });
      }),
    );

    const ok = await loadSessionHistory('session-1');

    expect(ok).toBe(true);
    expect(calls).toBe(1); // 不再因"缓存非空"跳过历史拉取
    const st = useAppStore.getState();
    const msgs = st.sessionMessages['session-1'] ?? [];
    expect(msgs).toHaveLength(3); // 2 条历史 + 1 条流式残留
    expect(msgs[msgs.length - 1]?.id).toBe('live-1'); // 流式尾部不被覆盖丢失
    expect(st.sessionMessageMeta['session-1']).toEqual({ oldestSeq: 12, hasMore: true });
  });

  it('applies the snapshot turn authority to turnState (ADR-045)', () => {
    // 残留 running（丢过 turn_state 事件）→ 快照权威值校正为 idle
    useAppStore.getState().setTurnState('session-1', { state: 'running', auto: true });

    act(() => {
      applySnapshot('session-1', historyMessages(), {
        oldestSeq: 12,
        hasMore: true,
        turn: { state: 'idle', auto: false },
      });
    });

    expect(useAppStore.getState().turnState['session-1']).toEqual({
      state: 'idle',
      auto: false,
    });
  });

  it('loadOlder requests the older page via before_seq cursor and prepends it', async () => {
    useAppStore.getState().setSessionMessages('session-1', historyMessages());
    useAppStore.getState().setSessionMessageMeta('session-1', { oldestSeq: 12, hasMore: true });
    let capturedUrl = '';
    server.use(
      http.get(`${API_BASE}/sessions/session-1/messages`, ({ request }) => {
        capturedUrl = request.url;
        return HttpResponse.json({
          session_id: 'session-1',
          messages: [
            {
              id: 'msg-0',
              role: 'user',
              segments: [{ type: 'text', text: '更早的消息' }],
              timestamp: '',
            },
          ],
          has_more: false,
        });
      }),
    );

    const { result } = renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await result.current.loadOlder('session-1');
    });

    expect(capturedUrl).toContain('before_seq=12');
    expect(capturedUrl).toContain('limit=50');
    const msgs = useAppStore.getState().sessionMessages['session-1'] ?? [];
    expect(msgs).toHaveLength(3);
    expect(msgs[0]?.id).toBe('msg-0');
    expect(useAppStore.getState().sessionMessageMeta['session-1']).toEqual({
      oldestSeq: null,
      hasMore: false,
    });
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
      .setSessionMessages('session-1', [
        { role: 'user', segments: [{ type: 'text', text: 'old' }], timestamp: '' },
      ]);

    const { result } = renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await result.current.reloadSession('session-1');
    });

    expect(useAppStore.getState().sessionMessages['session-1']).toEqual([]);
  });

  it('subscribes only once under StrictMode double-mount (forced-set guard)', async () => {
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
