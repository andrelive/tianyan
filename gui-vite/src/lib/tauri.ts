/**
 * Tauri 桌面壳桥接（可选能力）：GUI 同时运行在浏览器（独立服务模式/E2E）
 * 与 Tauri WebView（桌面应用）两种环境，桌面专属能力经此单点探测/调用，
 * 浏览器环境调用方自行回退（如手动输入路径）。
 */

/** 是否运行在 Tauri 桌面壳内（WebView 注入了 Tauri IPC）。 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * 原生目录选择对话框（Tauri 桌面壳；依赖 tauri-plugin-dialog）。
 * 非 Tauri 环境返回 null——调用方回退到手动输入路径。
 */
export async function pickDirectory(title = '选择目录'): Promise<string | null> {
  if (!isTauri()) return null;
  // 动态 import：浏览器构建不打包插件代码，桌面壳内才加载
  const mod = await import('@tauri-apps/plugin-dialog');
  const selected = await mod.open({
    directory: true,
    multiple: false,
    title,
  });
  return typeof selected === 'string' && selected.length > 0 ? selected : null;
}
