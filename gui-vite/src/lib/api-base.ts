import { useAppStore } from '@/lib/store';

/**
 * store 里 `apiBaseUrl` 的出厂默认值。
 *
 * 它同时是「用户没有显式配置过」的判定标记：等于这个值说明用户没动过设置，
 * 生产环境应按**同源**处理（见 [`resolveProdBase`]）。
 */
const DEFAULT_API_BASE_URL = 'http://localhost:3000';

/**
 * 生产环境（非 DEV、非注入）下的基址解析。
 *
 * **为什么不能直接信 store 的默认值**：`apiBaseUrl` 出厂是
 * `http://localhost:3000`，而后端实际端口可以变（`TIANYAN_PORT` / 端口冲突动态选）。
 * 页面本身既然由后端提供（`http(s)://…`），**同源**才是唯一可靠的默认——
 * 否则 headless 独立启动在非 3000 端口会整页白屏（CORS 拒绝 config/status）。
 *
 * 优先级：
 * 1. 用户显式改过设置（`apiBaseUrl !== DEFAULT_API_BASE_URL`）→ 尊重用户（跨源部署场景）；
 * 2. 页面由 http(s) 提供 → `window.location.origin`（同源，零 CORS）；
 * 3. 其余（`file://` / `tauri://` 等非 http 场景）→ 回退 store 值（桌面端本就由
 *    tauri 注入 `__TIANYAN_API_BASE__`，走不到这里）。
 */
function resolveProdBase(): string {
  const configured = useAppStore.getState().apiBaseUrl;
  if (configured && configured !== DEFAULT_API_BASE_URL) {
    return configured;
  }
  const origin = window.location.origin;
  if (origin.startsWith('http')) {
    return origin;
  }
  return configured || origin;
}

/** API 根 URL（不含 /api/v1；健康检查等非 API 端点用）。 */
export function getApiRoot(): string {
  const injected = (window as { __TIANYAN_API_BASE__?: string }).__TIANYAN_API_BASE__;
  if (injected) return injected;
  if (import.meta.env.DEV) return '';
  return resolveProdBase();
}

/** API base URL 解析（原 hooks/use-api-base：不是 hook，且被 lib 层引用——移入 lib）。 */
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
  // Production: 同源优先（见 resolveProdBase），再拼 /api/v1
  return `${resolveProdBase()}/api/v1`;
}
