import { useAppStore } from '@/lib/store';

export function getApiBase(): string {
  // Tauri 桌面端注入的实际监听端口（首选 3000 被占用时动态选择，
  // 此时默认值不可用，必须使用注入值）
  const injected = (window as { __TIANYAN_API_BASE__?: string })
    .__TIANYAN_API_BASE__;
  if (injected) {
    return `${injected}/api/v1`;
  }
  // Dev mode with Vite proxy: use relative path
  const href = window.location.href;
  if (href.includes('localhost:5173') || href.includes('127.0.0.1:5173')) {
    return '/api/v1';
  }
  // Production: read base URL from store and append /api/v1
  const base = useAppStore.getState().apiBaseUrl;
  return `${base}/api/v1`;
}
