import '@testing-library/jest-dom/vitest';
import { server } from './mocks/server';
import { cleanup } from '@testing-library/react';

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
