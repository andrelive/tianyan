import { describe, it, expect, vi, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useKeyboardShortcuts } from '../use-keyboard-shortcuts';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useKeyboardShortcuts', () => {
  it('handles Ctrl/Cmd+N, Ctrl+Shift+Delete and Ctrl+, without duplicates', () => {
    const handlers = { onNewChat: vi.fn(), onClearMessages: vi.fn(), onOpenSettings: vi.fn() };
    renderHook(() => useKeyboardShortcuts(handlers));

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'n', ctrlKey: true }));
    });
    expect(handlers.onNewChat).toHaveBeenCalledTimes(1);

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'n', metaKey: true }));
    });
    expect(handlers.onNewChat).toHaveBeenCalledTimes(2);

    act(() => {
      window.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Delete', ctrlKey: true, shiftKey: true }),
      );
    });
    expect(handlers.onClearMessages).toHaveBeenCalledTimes(1);

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: ',', ctrlKey: true }));
    });
    expect(handlers.onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it('ignores bare keys and wrong modifiers', () => {
    const handlers = { onNewChat: vi.fn(), onClearMessages: vi.fn(), onOpenSettings: vi.fn() };
    renderHook(() => useKeyboardShortcuts(handlers));

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'n' }));
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', ctrlKey: true }));
      window.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'n', ctrlKey: true, shiftKey: true }),
      );
      window.dispatchEvent(new KeyboardEvent('keydown', { key: ',', shiftKey: true }));
    });
    expect(handlers.onNewChat).not.toHaveBeenCalled();
    expect(handlers.onClearMessages).not.toHaveBeenCalled();
    expect(handlers.onOpenSettings).not.toHaveBeenCalled();
  });

  it('removes the listener on unmount', () => {
    const handlers = { onNewChat: vi.fn(), onClearMessages: vi.fn(), onOpenSettings: vi.fn() };
    const { unmount } = renderHook(() => useKeyboardShortcuts(handlers));
    unmount();

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'n', ctrlKey: true }));
    });
    expect(handlers.onNewChat).not.toHaveBeenCalled();
  });
});
