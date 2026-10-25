import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

/** 顶层 mock：@tauri-apps/plugin-opener 只被"Tauri 环境分支"动态加载。 */
const openUrl = vi.fn().mockResolvedValue(undefined);
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

/** 文件对话框插件（pickFiles 的底层）——同样只在桌面环境分支被动态加载。 */
const openDialog = vi.fn();
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: openDialog }));

/** webview 原生拖放订阅（onFileDrop 的底层）。 */
const onDragDropEvent = vi.fn();
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent }),
}));

/** IPC 核心（readClipboardFiles 的底层：自定义 command 走 invoke）。 */
const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

import { isTauri, onFileDrop, openExternal, pickFiles, readClipboardFiles } from '../tauri';

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

/**
 * 文件引用通道的原生入口：桌面版拿绝对路径的**唯一**两条路。
 * 浏览器环境必须退化为空结果/空操作（调用方无需分支，E2E 在浏览器里跑）。
 */
describe('tauri bridge / pickFiles', () => {
  beforeEach(() => {
    openDialog.mockReset();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it('非 Tauri 环境：返回空数组且不加载对话框插件', async () => {
    await expect(pickFiles()).resolves.toEqual([]);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('Tauri 环境：多选文件（非目录）并把返回值归一成数组', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    openDialog.mockResolvedValue(['C:\\a\\b.ts', 'C:\\a\\c.md']);
    await expect(pickFiles()).resolves.toEqual(['C:\\a\\b.ts', 'C:\\a\\c.md']);
    expect(openDialog).toHaveBeenCalledWith(
      expect.objectContaining({ multiple: true, directory: false }),
    );

    // 单选用例形态（string）与取消（null）都要归一
    openDialog.mockResolvedValue('C:\\only\\one.rs');
    await expect(pickFiles()).resolves.toEqual(['C:\\only\\one.rs']);
    openDialog.mockResolvedValue(null);
    await expect(pickFiles()).resolves.toEqual([]);
  });
});

describe('tauri bridge / onFileDrop', () => {
  beforeEach(() => {
    onDragDropEvent.mockReset();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it('非 Tauri 环境：返回空操作，不订阅原生事件', async () => {
    const cb = vi.fn();
    const unlisten = await onFileDrop(cb);
    expect(typeof unlisten).toBe('function');
    expect(() => unlisten()).not.toThrow();
    expect(onDragDropEvent).not.toHaveBeenCalled();
  });

  it('Tauri 环境：只把 drop 的 paths 交给回调，并返回退订函数', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const unlistenMock = vi.fn();
    let handler: ((e: { payload: { type: string; paths?: string[] } }) => void) | null = null;
    onDragDropEvent.mockImplementation(
      async (h: (e: { payload: { type: string; paths?: string[] } }) => void) => {
        handler = h;
        return unlistenMock;
      },
    );

    const cb = vi.fn();
    const unlisten = await onFileDrop(cb);
    expect(handler).not.toBeNull();

    // 悬停态不该触发（调用方只在真正放下时才追加附件）
    handler!({ payload: { type: 'over' } });
    expect(cb).not.toHaveBeenCalled();

    handler!({ payload: { type: 'drop', paths: ['C:\\a\\b.ts'] } });
    expect(cb).toHaveBeenCalledWith(['C:\\a\\b.ts']);

    expect(unlisten).toBe(unlistenMock);
    unlisten();
    expect(unlistenMock).toHaveBeenCalled();
  });
});
