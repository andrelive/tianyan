/**
 * 流响应看门狗测试（T1-8 自愈）。
 *
 * 覆盖：超时复位 streaming / 持续喂狗不误判 / 卡住的 running 轮复位。
 * 背景：收尾事件依赖事件必达（有界通道 Lag 丢弃 + SSE 断线全丢），
 * 缺失时前端会永久卡 streaming/running（实测 300s+ 无响应观感）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  __resetStreamWatchdog,
  startStreamWatchdog,
  STREAM_IDLE_TIMEOUT_MS,
  touchStreamActivity,
} from '@/lib/stream-watchdog';
import { useAppStore } from '@/lib/store';

describe('stream-watchdog (T1-8)', () => {
  let stop: () => void;

  beforeEach(() => {
    vi.useFakeTimers();
    __resetStreamWatchdog();
    useAppStore.setState({
      streamStatus: {},
      turnState: {},
      messages: [],
      sessionMessages: {},
    });
    stop = startStreamWatchdog();
  });

  afterEach(() => {
    stop();
    vi.useRealTimers();
  });

  it('resets a stale streaming session (收尾事件丢失时的兜底)', () => {
    useAppStore.setState({ streamStatus: { s1: 'streaming' } });
    // 首见只记起点；超过阈值后复位
    vi.advanceTimersByTime(STREAM_IDLE_TIMEOUT_MS + 60_000);
    expect(useAppStore.getState().streamStatus.s1 ?? 'idle').toBe('idle');
  });

  it('does not reset while events keep arriving (不误判活跃流)', () => {
    useAppStore.setState({ streamStatus: { s1: 'streaming' } });
    vi.advanceTimersByTime(STREAM_IDLE_TIMEOUT_MS - 30_000);
    touchStreamActivity('s1');
    vi.advanceTimersByTime(STREAM_IDLE_TIMEOUT_MS - 30_000);
    expect(useAppStore.getState().streamStatus.s1).toBe('streaming');
  });

  it('resets a stale running turn (唤醒轮卡住时解锁输入区)', () => {
    useAppStore.setState({ turnState: { s1: { state: 'running', auto: true } } });
    vi.advanceTimersByTime(STREAM_IDLE_TIMEOUT_MS + 60_000);
    expect(useAppStore.getState().turnState.s1?.state ?? 'idle').toBe('idle');
  });
});
