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

/**
 * 用系统默认应用打开外部链接（Tauri 桌面壳；依赖 tauri-plugin-opener）。
 *
 * 返回值表示"是否已接管"：
 * - `true`：桌面壳已把链接交给系统默认浏览器——调用方应 `preventDefault()`
 *   阻止 WebView 自行导航（WebView 导航外部 URL 无回退入口，失败即白屏）；
 * - `false`：非 Tauri 环境（浏览器 / E2E）未接管——调用方保持浏览器默认
 *   行为，与改造前完全一致。
 */
export async function openExternal(url: string): Promise<boolean> {
  if (!isTauri()) return false;
  // 动态 import：浏览器构建不打包插件代码，桌面壳内才加载
  const mod = await import('@tauri-apps/plugin-opener');
  await mod.openUrl(url);
  return true;
}

/**
 * 读取系统剪贴板里的**文件路径**（Tauri 桌面壳；自定义 command
 * `read_clipboard_files`，Rust 侧直接读 Windows `CF_HDROP` 文件列表）。
 *
 * **为什么必须走原生**：Web 层分不清「复制的文件」和「截图」。两者在
 * `paste` 事件里都是 `kind: 'file'` 的项（`Win+Shift+S` 截图的 MIME 同样是
 * `image/png`），而 `DataTransferItem.getAsFile()` 给的 `File` 也没有绝对
 * 路径（`File.path` 是 Electron 扩展，Web 标准没有）。只有原生侧能回答
 * 「剪贴板里到底有没有文件列表」这个问题，前端据此分流到文件 chip 通道或
 * 图片通道。
 *
 * **为什么失败一律降级成空数组**：本函数由粘贴事件驱动，失败是正常场景——
 * Windows 上剪贴板常被其他进程短暂占用（`OpenClipboard` 争用）、IPC 可能
 * 中断、未来 ACL 收紧也会让 invoke 直接报错。抛异常会让「粘贴」这个动作
 * 完全没反应；返回 `[]` 时调用方会安静回退到图片通道（截图）或文本默认
 * 粘贴，用户至少不会遇到「按了 Ctrl+V 什么都没发生」。
 */
export async function readClipboardFiles(): Promise<string[]> {
  // 浏览器 / E2E 环境没有原生剪贴板文件通道：直接空结果，不加载 IPC 代码
  if (!isTauri()) return [];
  try {
    // 动态 import：浏览器构建不打包 IPC 调用代码，桌面壳内才加载
    const mod = await import('@tauri-apps/api/core');
    const paths = await mod.invoke<string[]>('read_clipboard_files');
    // 防御 IPC 返回非数组（版本漂移时不让调用方拿到 undefined 而崩在 .length 上）
    return Array.isArray(paths) ? paths : [];
  } catch {
    return [];
  }
}
