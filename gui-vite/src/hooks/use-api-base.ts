import { useAppStore } from '@/lib/store';

export function getApiBase(): string {
  // Dev mode with Vite proxy: use relative path
  const href = window.location.href;
  if (href.includes('localhost:5173') || href.includes('127.0.0.1:5173')) {
    return '/api/v1';
  }
  // Production: read base URL from store and append /api/v1
  const base = useAppStore.getState().apiBaseUrl;
  return `${base}/api/v1`;
}
