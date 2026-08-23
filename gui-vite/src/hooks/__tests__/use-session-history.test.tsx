import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import { StrictMode, type ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '@/test/mocks/server';
import { useAppStore } from '@/lib/store';
import { useSessionHistory } from '../use-session-history';

const API_BASE = '/api/v1';

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('useSessionHistory', () => {
  it('loads messages on mount for the URL session and writes the store projection', async () => {
    renderHook(() => useSessionHistory('session-1'));

    await waitFor(() => {
      const msgs = useAppStore.getState().messages;
      expect(msgs).toHaveLength(2);
      expect(msgs[0]?.content).toBe('你好');
      expect(msgs[1]?.role).toBe('assistant');
    });
  });

  it('skips the fetch when local cache already exists', async () => {
    useAppStore
      .getState()
      .setSessionMessages('session-1', [{ role: 'user', content: 'cached', timestamp: '' }]);
    useAppStore.getState().setCurrentSession('session-1');

    renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await Promise.resolve();
    });

    // 缓存直接使用：不发起请求，投影未被覆盖
    expect(useAppStore.getState().messages[0]?.content).toBe('cached');
  });

  it('does not apply a stale response after the URL session changed mid-flight', async () => {
    let resolveSlow!: (value: Response) => void;
    server.use(
      http.get(`${API_BASE}/sessions/slow-1/messages`, () => {
        return new Promise<Response>((resolve) => {
          resolveSlow = resolve;
        });
      }),
    );

    const { rerender } = renderHook(({ id }: { id: string }) => useSessionHistory(id), {
      initialProps: { id: 'slow-1' },
    });
    await act(async () => {
      await Promise.resolve();
    });

    // 请求在途时 URL 已切走 → 旧响应到达后不得应用
    rerender({ id: 'session-2' });
    act(() => {
      resolveSlow(
        HttpResponse.json({
          session_id: 'slow-1',
          messages: [{ role: 'assistant', content: 'stale', timestamp: '' }],
        }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(useAppStore.getState().messages).toHaveLength(0);
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
      .setSessionMessages('session-1', [{ role: 'user', content: 'old', timestamp: '' }]);

    const { result } = renderHook(() => useSessionHistory('session-1'));
    await act(async () => {
      await result.current.reloadSession('session-1');
    });

    expect(useAppStore.getState().sessionMessages['session-1']).toEqual([]);
  });

  it('fetches only once under StrictMode double-mount (ref guard)', async () => {
    const fetchCalls = vi.fn();
    server.use(
      http.get(`${API_BASE}/sessions/session-1/messages`, () => {
        fetchCalls();
        return HttpResponse.json({ session_id: 'session-1', messages: [] });
      }),
    );
    const wrapper = ({ children }: { children: ReactNode }) => <StrictMode>{children}</StrictMode>;

    renderHook(() => useSessionHistory('session-1'), { wrapper });
    await act(async () => {
      await Promise.resolve();
    });

    expect(fetchCalls).toHaveBeenCalledTimes(1);
  });
});
