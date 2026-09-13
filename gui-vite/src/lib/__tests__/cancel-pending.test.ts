import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { cancelWhenSessionIdReady } from '@/lib/cancel-pending';

describe('cancelWhenSessionIdReady', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('会话 id 稍后就绪 → 补发一次取消（竞态窗口内点停止不再丢失）', () => {
    let sessionId: string | null = null;
    const sent: string[] = [];
    cancelWhenSessionIdReady(
      () => sessionId,
      (id) => sent.push(id),
      { intervalMs: 250, maxAttempts: 40 },
    );

    // 前两次轮询仍无 id（请求还没返回）
    vi.advanceTimersByTime(250);
    vi.advanceTimersByTime(250);
    expect(sent).toEqual([]);

    // 请求返回、会话 id 就绪 → 下一次轮询立即补发
    sessionId = 'session-xyz';
    vi.advanceTimersByTime(250);
    expect(sent).toEqual(['session-xyz']);

    // 已补发：后续轮询不再重复发送
    vi.advanceTimersByTime(250 * 5);
    expect(sent).toEqual(['session-xyz']);
  });

  it('会话 id 始终未出现 → 达到上限后停止轮询（不误发、不泄漏）', () => {
    const sent: string[] = [];
    cancelWhenSessionIdReady(
      () => null,
      (id) => sent.push(id),
      {
        intervalMs: 100,
        maxAttempts: 3,
      },
    );

    vi.advanceTimersByTime(100 * 10);
    expect(sent).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('清理函数能停止轮询（组件卸载/新一轮开始）', () => {
    let sessionId: string | null = null;
    const sent: string[] = [];
    const dispose = cancelWhenSessionIdReady(
      () => sessionId,
      (id) => sent.push(id),
      { intervalMs: 100, maxAttempts: 10 },
    );

    dispose();
    sessionId = 'session-late';
    vi.advanceTimersByTime(100 * 5);

    expect(sent).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });
});
