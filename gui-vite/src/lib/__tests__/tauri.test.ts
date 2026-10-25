import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

/** 顶层 mock：@tauri-apps/plugin-opener 只被"Tauri 环境分支"动态加载。 */
const openUrl = vi.fn().mockResolvedValue(undefined);
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

/** IPC 核心（readClipboardFiles 的底层：自定义 command 走 invoke）。 */
const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

import { isTauri, openExternal, readClipboardFiles } from '../tauri';

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

/**
 * 剪贴板文件通道：Web 层分不清「复制的文件」与「截图」（都是 `kind: 'file'`，
 * 截图 MIME 也是 image/png），只有原生侧能回答「剪贴板里有没有文件列表」。
 * 本函数任何失败都必须降级成空数组——调用方据空数组回退图片通道。
 */
describe('tauri bridge / readClipboardFiles', () => {
  beforeEach(() => {
    invoke.mockReset();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it('非 Tauri 环境：返回空数组且不加载 IPC 代码', async () => {
    await expect(readClipboardFiles()).resolves.toEqual([]);
    expect(invoke).not.toHaveBeenCalled();
  });

  it('Tauri 环境：调用 read_clipboard_files 并透传路径列表', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invoke.mockResolvedValue(['C:\\work\\a.pdf', 'C:\\work\\b.ts']);

    await expect(readClipboardFiles()).resolves.toEqual(['C:\\work\\a.pdf', 'C:\\work\\b.ts']);
    expect(invoke).toHaveBeenCalledWith('read_clipboard_files');
  });

  it('invoke 抛错（ACL 拒绝 / 剪贴板争用 / IPC 中断）：降级成空数组而不是抛出', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invoke.mockRejectedValue(new Error('Command read_clipboard_files not allowed by ACL'));

    await expect(readClipboardFiles()).resolves.toEqual([]);
  });
});
