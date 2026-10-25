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
 * 原生**多选文件**对话框（Tauri 桌面壳；依赖 tauri-plugin-dialog）。
 *
 * 返回用户选中的**绝对路径**列表（取消 / 空选 → `[]`）。浏览器环境没有
 * 绝对路径可言（HTML `File` 对象只有 name/size，`path` 是 Electron 扩展），
 * 直接返回 `[]`——调用方回退到「把路径贴进输入框」。
 */
export async function pickFiles(title = '选择文件'): Promise<string[]> {
  if (!isTauri()) return [];
  // 动态 import：浏览器构建不打包插件代码，桌面壳内才加载
  const mod = await import('@tauri-apps/plugin-dialog');
  const selected = await mod.open({ multiple: true, directory: false, title });
  if (typeof selected === 'string') return [selected];
  return Array.isArray(selected) ? selected : [];
}

/**
 * 订阅**原生文件拖放**（Tauri 桌面壳；`@tauri-apps/api/webview`）。
 *
 * 桌面版必须走这条通道：`tauri.conf.json` 的 `drag_drop_enabled` 默认 true，
 * Tauri 在 WebView 层**拦截**了系统拖放，前端的 HTML5 `drop` 事件收不到文件
 * （Windows 尤其如此）；而 HTML5 的 `File` 对象也拿不到绝对路径。原生事件
 * 回调里的 `paths` 才是真路径。
 *
 * 只回调 `drop`（`enter` / `over` / `leave` 是悬停态，调用方不需要）；
 * 返回取消订阅函数（组件卸载时调用）。浏览器环境返回空操作——调用方无需
 * 分支判断。
 */
export async function onFileDrop(cb: (paths: string[]) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  // 动态 import：浏览器构建不打包桌面代码，E2E（浏览器）不受影响
  const mod = await import('@tauri-apps/api/webview');
  const unlisten = await mod.getCurrentWebview().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === 'drop') cb(payload.paths);
  });
  return unlisten;
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
