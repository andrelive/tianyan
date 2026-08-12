import { useAppStore } from '@/lib/store';

export function getApiBase(): string {
  // Tauri 桌面端注入的实际监听端口（首选 3000 被占用时动态选择，
  // 此时默认值不可用，必须使用注入值）
  const injected = (window as { __TIANYAN_API_BASE__?: string }).__TIANYAN_API_BASE__;
  if (injected) {
    return `${injected}/api/v1`;
  }
  // Dev mode with Vite proxy: use relative path（与端口无关——
  // 修复：原实现硬编码 5173，端口变化（如 5174）时误走生产分支导致跨源 CORS 失败）
  if (import.meta.env.DEV) {
    return '/api/v1';
  }
  // Production: read base URL from store and append /api/v1
  const base = useAppStore.getState().apiBaseUrl;
  return `${base}/api/v1`;
}
