import { useEffect } from 'react';

interface ShortcutHandlers {
  onNewChat: () => void;
  onClearMessages: () => void;
  onOpenSettings: () => void;
}

export function useKeyboardShortcuts(handlers: ShortcutHandlers) {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
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
