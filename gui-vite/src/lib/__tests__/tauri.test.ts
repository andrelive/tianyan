import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

/** 顶层 mock：@tauri-apps/plugin-opener 只被"Tauri 环境分支"动态加载。 */
const openUrl = vi.fn().mockResolvedValue(undefined);
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

import { isTauri, openExternal } from '../tauri';

/**
 * B 方案兜底的环境契约：桌面壳内接管（交给系统浏览器），
 * 浏览器 / E2E 环境不接管（保持默认行为，调用方不 preventDefault）。
 */
describe('tauri bridge / openExternal', () => {
  beforeEach(() => {
    openUrl.mockClear();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it('非 Tauri 环境（浏览器 / E2E）：不接管且不加载插件', async () => {
    expect(isTauri()).toBe(false);
    await expect(openExternal('https://example.com')).resolves.toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
  });

  it('Tauri 环境：调用 opener 打开系统默认浏览器', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    expect(isTauri()).toBe(true);
    await expect(openExternal('https://example.com')).resolves.toBe(true);
    expect(openUrl).toHaveBeenCalledWith('https://example.com');
  });
});
