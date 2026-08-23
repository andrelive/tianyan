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
  it('streams SSE events into the store and completes with the stream session', async () => {
    const onComplete = vi.fn();
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useChatStream({
        streamUrl: '/api/v1/chat/stream',
        onComplete,
        onError,
      }),
    );

    // 与 ChatPanel.handleSend 一致：先放 user 消息 + assistant 占位，
    // 流式增量累积在占位上（归约器不自行创建消息）
    act(() => {
      useAppStore.getState().addMessage({ role: 'user', content: '你好', timestamp: '' });
      useAppStore.getState().addMessage({ role: 'assistant', content: '', timestamp: '' });
    });

    await act(async () => {
      await result.current.startStream({
        session_id: 'session-1',
        message: { role: 'user', content: '你好' },
        stream: true,
        temperature: 0.7,
        max_tokens: 2048,
      });
    });

    expect(onError).not.toHaveBeenCalled();
    expect(onComplete).toHaveBeenCalledWith('session-1');
    // MSW 流式 fixture：answer delta「你好」「！」累积到 assistant 消息
    const messages = useAppStore.getState().messages;
    const assistant = messages.find((m) => m.role === 'assistant');
    expect(assistant?.content).toContain('你好');
    expect(assistant?.content).toContain('！');
  });

  it('reports non-OK responses through onError without a session', async () => {
    server.use(http.post('/api/v1/chat/stream', () => new HttpResponse(null, { status: 500 })));
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useChatStream({
        streamUrl: '/api/v1/chat/stream',
        onError,
      }),
    );

    await act(async () => {
      await result.current.startStream({
        session_id: 'session-1',
        message: { role: 'user', content: '你好' },
        stream: true,
        temperature: 0.7,
        max_tokens: 2048,
      });
    });

    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0]).toBeNull(); // 无事件 → streamSessionId 为 null
  });

  it('stopStream aborts an in-flight stream and completes without error', async () => {
    const onComplete = vi.fn();
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useChatStream({
        streamUrl: '/api/v1/chat/stream',
        onComplete,
        onError,
      }),
    );

    let started!: Promise<void>;
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

    expect(onError).not.toHaveBeenCalled();
    expect(onComplete).toHaveBeenCalledTimes(1);
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
