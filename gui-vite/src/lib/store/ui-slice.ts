/**
 * UI 切片（uiSlice）—— 导航/外观/通知/启动状态。
 */

import type { StateCreator } from 'zustand';
import type { FontSize, Theme, ToastMessage, View } from '@/lib/types';

export interface UiSlice {
  // View
  currentView: View;
  setView: (view: View) => void;

  // Sidebar
  isSidebarOpen: boolean;
  toggleSidebar: () => void;
  setSidebarOpen: (open: boolean) => void;

  // Settings（本地偏好，persist 持久化 theme/fontSize）
  theme: Theme;
  fontSize: FontSize;
  apiBaseUrl: string;
  setTheme: (theme: Theme) => void;
  setFontSize: (size: FontSize) => void;
  setApiBaseUrl: (url: string) => void;

  // Toast
  toast: ToastMessage | null;
  showToast: (message: string, type: ToastMessage['type']) => void;
  hideToast: () => void;

  // App mode
  configured: boolean | null;
  setConfigured: (val: boolean) => void;
}

export const createUiSlice: StateCreator<UiSlice, [], [], UiSlice> = (set) => ({
  // View
  currentView: 'chat',
  setView: (view) => set({ currentView: view }),

  // Sidebar
  isSidebarOpen: true,
  toggleSidebar: () => set((s) => ({ isSidebarOpen: !s.isSidebarOpen })),
  setSidebarOpen: (open) => set({ isSidebarOpen: open }),

  // Settings
  theme: 'system',
  fontSize: 'medium',
  apiBaseUrl: 'http://localhost:3000',
  setTheme: (theme) => set({ theme }),
  setFontSize: (size) => set({ fontSize: size }),
  setApiBaseUrl: (url) => set({ apiBaseUrl: url }),

  // Toast
  toast: null,
  showToast: (message, type) => set({ toast: { message, type } }),
  hideToast: () => set({ toast: null }),

  // App mode
  configured: null,
  setConfigured: (val) => set({ configured: val }),
});
