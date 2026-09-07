import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '@/test/mocks/server';
import { useAppStore } from '@/lib/store';
import { useResource } from '../use-resource';
import { usePolling } from '../use-polling';
import { useChatStream } from '../useChatStream';

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('useResource', () => {
  it('loads data on mount and exposes loading/error transitions', async () => {
    const fetcher = vi.fn().mockResolvedValue('ok');
    const { result } = renderHook(() => useResource(fetcher, []));

    expect(result.current.loading).toBe(true);
    await waitFor(() => {
      expect(result.current.data).toBe('ok');
    });
    expect(result.current.loading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('captures error message on rejection', async () => {
    const fetcher = vi.fn().mockRejectedValue(new Error('boom'));
    const { result } = renderHook(() => useResource(fetcher, []));

    await waitFor(() => {
      expect(result.current.error).toBe('boom');
    });
    expect(result.current.data).toBeNull();
  });

  it('reload() re-runs the fetcher', async () => {
    let value = 1;
    const fetcher = vi.fn().mockImplementation(async () => value);
    const { result } = renderHook(() => useResource(fetcher, []));

    await waitFor(() => {
      expect(result.current.data).toBe(1);
    });
    value = 2;
    act(() => {
      result.current.reload();
    });
    await waitFor(() => {
      expect(result.current.data).toBe(2);
    });
    expect(fetcher).toHaveBeenCalledTimes(2);
  });
});

describe('usePolling', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('runs immediately (default) and on interval', async () => {
    vi.useFakeTimers();
    const fn = vi.fn().mockResolvedValue(undefined);
    renderHook(() => usePolling(fn, 1000));

    expect(fn).toHaveBeenCalledTimes(1); // immediate
    await act(async () => {
      vi.advanceTimersByTime(3000);
    });
    expect(fn).toHaveBeenCalledTimes(4);
  });

  it('skips immediate run and pauses when disabled', () => {
    vi.useFakeTimers();
    const fn = vi.fn().mockResolvedValue(undefined);
    renderHook(() => usePolling(fn, 1000, { immediate: false, enabled: false }));

    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(fn).not.toHaveBeenCalled();
  });

  it('routes errors to onError', async () => {
    vi.useFakeTimers();
    const onError = vi.fn();
    const fn = vi.fn().mockRejectedValue(new Error('poll-fail'));
    renderHook(() => usePolling(fn, 1000, { immediate: false, onError }));

    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    expect(onError).toHaveBeenCalledWith(expect.any(Error));
  });

  it('exposes refresh() for manual trigger', async () => {
    const fn = vi.fn().mockResolvedValue(undefined);
    const { result } = renderHook(() => usePolling(fn, 60000, { immediate: false }));

    await act(async () => {
      await result.current.refresh();
    });
    expect(fn).toHaveBeenCalledTimes(1);
  });
});

describe('useChatStream', () => {
  it('starts the loop and returns the confirmed session id', async () => {
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useChatStream({
        streamUrl: '/api/v1/chat/stream',
        onError,
      }),
    );

    let sid: string | null = null;
    await act(async () => {
      sid = await result.current.startStream({
        session_id: 'session-1',
        message: { role: 'user', content: '你好' },
        stream: true,
        temperature: 0.7,
        max_tokens: 2048,
      });
    });

    expect(onError).not.toHaveBeenCalled();
    // ADR-028 第 3 步：启动请求立即返回服务端确认的 session_id
    expect(sid).toBe('session-1');
  });

  it('reports non-OK responses through onError', async () => {
    server.use(http.post('/api/v1/chat/stream', () => new HttpResponse(null, { status: 500 })));
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useChatStream({
        streamUrl: '/api/v1/chat/stream',
        onError,
      }),
    );

    let sid: string | null = 'x';
    await act(async () => {
      sid = await result.current.startStream({
        session_id: 'session-1',
        message: { role: 'user', content: '你好' },
        stream: true,
        temperature: 0.7,
        max_tokens: 2048,
      });
    });

    expect(onError).toHaveBeenCalledTimes(1);
    expect(sid).toBeNull(); // 启动失败 → 无 session_id
  });

  it('stopStream aborts an in-flight start request without error', async () => {
    // 延迟响应：让 abort 在 fetch 挂起时生效（MSW 默认立即返回，abort 来不及）
    server.use(
      http.post('/api/v1/chat/stream', async () => {
        await new Promise((r) => setTimeout(r, 50));
        return HttpResponse.json({ status: 'started', session_id: 'session-1' });
      }),
    );
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useChatStream({
        streamUrl: '/api/v1/chat/stream',
        onError,
      }),
    );

    let started!: Promise<string | null>;
    act(() => {
      started = result.current.startStream({
        session_id: 'session-1',
        message: { role: 'user', content: '你好' },
        stream: true,
        temperature: 0.7,
        max_tokens: 2048,
      });
      result.current.stopStream();
    });
    await act(async () => {
      await started;
    });

    // 核心语义：stop 不触发 onError（abort 是否真正中断请求受测试环境限制——
    // jsdom AbortController 跨 realm 被 Node fetch 拒绝后降级为无 signal
    // 请求，abort 退化为无效操作，见 fetch-with-signal.ts）
    expect(onError).not.toHaveBeenCalled();
  });
});

describe('useResource enabled', () => {
  it('pauses while disabled and fetches when enabled flips', async () => {
    const fetcher = vi.fn().mockResolvedValue('ok');
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) => useResource(fetcher, [], { enabled }),
      { initialProps: { enabled: false } },
    );

    expect(fetcher).not.toHaveBeenCalled();
    expect(result.current.loading).toBe(false);
    expect(result.current.data).toBeNull();

    await act(async () => {
      rerender({ enabled: true });
    });
    await waitFor(() => {
      expect(result.current.data).toBe('ok');
    });
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(result.current.loading).toBe(false);
  });

  it('clears error and loading when disabled mid-flight', async () => {
    const fetcher = vi.fn().mockRejectedValue(new Error('boom'));
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) => useResource(fetcher, [], { enabled }),
      { initialProps: { enabled: true } },
    );

    await waitFor(() => {
      expect(result.current.error).toBe('boom');
    });

    await act(async () => {
      rerender({ enabled: false });
    });
    expect(result.current.loading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('refetches when deps change while enabled', async () => {
    const fetcher = vi.fn().mockImplementation(async (id: string) => 'detail-' + id);
    const { result, rerender } = renderHook(
      ({ id }: { id: string }) => useResource(() => fetcher(id), [id]),
      { initialProps: { id: 'a' } },
    );

    await waitFor(() => {
      expect(result.current.data).toBe('detail-a');
    });
    await act(async () => {
      rerender({ id: 'b' });
    });
    await waitFor(() => {
      expect(result.current.data).toBe('detail-b');
    });
    expect(fetcher).toHaveBeenCalledTimes(2);
  });
});
