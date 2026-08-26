import { useEffect } from 'react';

interface ShortcutHandlers {
  onNewChat: () => void;
  onClearMessages: () => void;
  onOpenSettings: () => void;
}

export function useKeyboardShortcuts(handlers: ShortcutHandlers) {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // 焦点在可编辑元素（输入框/文本域/内容可编辑）时忽略全局快捷键，
      // 避免在重命名/输入中误触（如 Ctrl+N 新建、Ctrl+Shift+Delete 清空）。
      const t = e.target as HTMLElement | null;
      const editable =
        t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
      if (editable) return;
      const ctrl = e.ctrlKey || e.metaKey;
      if (ctrl && !e.shiftKey && e.key === 'n') {
        e.preventDefault();
        handlers.onNewChat();
      } else if (ctrl && e.shiftKey && (e.key === 'Delete' || e.key === 'Backspace')) {
        e.preventDefault();
        handlers.onClearMessages();
      } else if (ctrl && !e.shiftKey && e.key === ',') {
        e.preventDefault();
        handlers.onOpenSettings();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [handlers]);
}
