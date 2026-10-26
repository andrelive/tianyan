import '@testing-library/jest-dom/vitest';
import { server } from './mocks/server';
import { cleanup } from '@testing-library/react';

/**
 * jsdom 没有布局引擎，`Range` 的几何测量 API 全部缺失（`Element` 的有，返回全 0）。
 *
 * Lexical 在提交一次编辑后会调用 `$updateDOMSelection` 恢复 DOM 选区，其中
 * `getCaretRect()` 用 `range.getBoundingClientRect()` 判断光标位置——jsdom 里
 * 这会抛 `TypeError: range.getBoundingClientRect is not a function`，并且**中断
 * 整次 commit**（文本停在编辑器模型里，永远到不了 DOM）。补一个「零矩形」替身
 * 即可让选区恢复逻辑跑完（坐标无意义，但单测不验证几何）。
 *
 * 只补 Range：单测不测布局，别顺手给所有几何 API 编造数值。
 */
function stubDOMRect(): DOMRect {
  return {
    x: 0,
    y: 0,
    width: 0,
    height: 0,
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    toJSON: () => ({}),
  } as DOMRect;
}

if (typeof Range !== 'undefined') {
  if (typeof Range.prototype.getBoundingClientRect !== 'function') {
    Range.prototype.getBoundingClientRect = stubDOMRect;
  }
  if (typeof Range.prototype.getClientRects !== 'function') {
    Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
  }
}

/**
 * jsdom 没有 `ClipboardEvent` 构造函数（只有 `Event`/`InputEvent` 等）。
 *
 * Lexical 的纯文本粘贴用 `objectKlassEquals(event, ClipboardEvent)` 判类型——
 * 这是一个**裸标识符**，全局缺失时直接抛 `ReferenceError: ClipboardEvent is not
 * defined`，粘贴文本在单测里就永远进不了编辑器。补一个最小实现（只关心
 * `clipboardData`）：真实浏览器里这个类由平台提供，polyfill 只在测试进程生效。
 *
 * 注意 `@testing-library/dom` 的 `fireEvent.paste` 会优先用 `window[EventType]`
 * 构造事件——补上它之后，测试里派发的 paste 事件才是「真的」ClipboardEvent
 * 实例，`objectKlassEquals` 才会命中。
 */
if (typeof (globalThis as { ClipboardEvent?: unknown }).ClipboardEvent === 'undefined') {
  class ClipboardEventPolyfill extends Event {
    readonly clipboardData: DataTransfer | null;

    constructor(type: string, init: ClipboardEventInit = {}) {
      super(type, init);
      this.clipboardData = init.clipboardData ?? null;
    }
  }
  Object.defineProperty(globalThis, 'ClipboardEvent', {
    value: ClipboardEventPolyfill,
    writable: true,
    configurable: true,
  });
}

// msw 保持 2.x：3.0 移除了 `onUnhandledRequest` 选项，且其异步管道与
// `vi.useFakeTimers()` 组合后不再完成（fake timers 下挂载请求悬挂 → 列表空）。
// 它只是测试工具，升级收益低而调试成本高，故与 lancedb 同型处理：暂缓升级
// （见 docs/operations/dependency-upgrade.md）。
beforeAll(() => server.listen({ onUnhandledRequest: 'warn' }));
afterEach(() => {
  server.resetHandlers();
  cleanup();
});
afterAll(() => server.close());
