import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { useAppStore } from '@/lib/store';
import {
  __resetSubscriptions,
  __resetUnifiedEventsForTest,
  __startUnifiedEventsForTest,
} from '../use-unified-events';

/**
 * T1-21 回归：常驻 EventSource 的**端口迁移**。
 *
 * 缺陷：内嵌服务重启后端口可能变化（tauri 重新注入 `__TIANYAN_API_BASE__`），
 * 而 `EventSource` 自动重连**永远复用创建时的 URL**——注入的新地址只对后续
 * fetch 生效，事件流永久失联（前端再收不到 chat_stream / task_status）。
 *
 * 修复：`onerror` 时比较当前 `getApiBase()` 与建连时的 base，变化则用新地址
 * 重建连接（订阅由 onopen 重放）；base 未变的瞬时断线交给浏览器自动重连。
 */

/** 最小 EventSource 替身：记录 URL 与关闭状态，供测试手动触发 onerror。 */
class FakeEventSource {
  static instances: FakeEventSource[] = [];
  url: string;
  closed = false;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  close(): void {
    this.closed = true;
  }
}

function current(): FakeEventSource {
  const es = FakeEventSource.instances[FakeEventSource.instances.length - 1];
  if (!es) throw new Error('未创建 EventSource');
  return es;
}

function setInjectedBase(base: string | undefined): void {
  (window as unknown as { __TIANYAN_API_BASE__?: string }).__TIANYAN_API_BASE__ = base;
}

beforeEach(() => {
  FakeEventSource.instances = [];
  useAppStore.setState(useAppStore.getInitialState());
  __resetSubscriptions();
  vi.stubGlobal('EventSource', FakeEventSource);
});

afterEach(() => {
  __resetUnifiedEventsForTest();
  setInjectedBase(undefined);
  vi.unstubAllGlobals();
});

describe('use-unified-events 端口迁移（T1-21）', () => {
  it('端口变化后 onerror 用新 base 重建连接，并关闭旧连接', () => {
    setInjectedBase('http://127.0.0.1:3000');
    __startUnifiedEventsForTest();
    const first = current();
    expect(first.url).toBe('http://127.0.0.1:3000/api/v1/events');

    // 内嵌服务重启后被外部抢占：tauri 重新注入新端口
    setInjectedBase('http://127.0.0.1:3001');
    first.onerror?.();

    const second = current();
    expect(second).not.toBe(first);
    expect(second.url).toBe('http://127.0.0.1:3001/api/v1/events');
    expect(first.closed).toBe(true);
  });

  it('base 未变的瞬时断线不重建（交给浏览器自动重连）', () => {
    setInjectedBase('http://127.0.0.1:3000');
    __startUnifiedEventsForTest();
    const first = current();

    first.onerror?.();

    expect(FakeEventSource.instances).toHaveLength(1);
    expect(first.closed).toBe(false);
    // T1-8：断线态标记仍生效
    expect(useAppStore.getState().eventsConnected).toBe(false);
  });
});
