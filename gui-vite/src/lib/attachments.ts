/**
 * 文件引用通道（批次 1）：把用户拖拽/选择的**任意非图片文件**表示为
 * 「绝对路径引用」——输入区只显示带图标的文件名 chip，真正发给模型的是
 * 路径文本行 `@file:"<绝对路径>"`，由模型侧自行 `read_file` 读取。
 *
 * 与图片通道（data URL base64）的分工：位图由 provider 直接看图，走
 * `ChatInput` 的 `images`；其他文件一律走本模块的路径引用。**后端零改动**：
 * 路径就是普通文本，`content` 里多几行 `@file:"..."` 而已。
 *
 * 本模块只做纯函数变换（可单测），不触碰 Tauri / 网络 / 组件状态。
 */

import {
  File,
  FileArchive,
  FileCode,
  FileImage,
  FileSpreadsheet,
  FileText,
  type LucideIcon,
} from 'lucide-react';

/** 一个待发送的文件引用：绝对路径（发送用）+ 文件名（展示用）。 */
export interface FileRef {
  /** 文件绝对路径（Windows：`F:\dir\a.ts`；POSIX：`/home/u/a.ts`）。 */
  path: string;
  /** 展示用文件名（basename；chip 只显示它，完整路径放 title）。 */
  name: string;
}

/**
 * 引用行的**唯一口径**：一行一个文件，路径用双引号包裹——Windows 路径
 * 可含空格（`C:\Program Files\a.txt`），不加引号就无法界定边界。
 */
const ATTACHMENT_LINE_RE = /^@file:"(.+)"$/;

/** 便宜的**前置判定**：不含该标记的文本直接返回原文，不逐行解析
 * （消息区每个文本段、流式每帧都会调用一次，不能做正则全扫）。 */
const ATTACHMENT_MARKER = '@file:"';

/** 从路径取文件名；分隔符兼容 `\` 与 `/`（上游多为 Windows，但代码不写死）。 */
export function basenameOf(path: string): string {
  const trimmed = path.trim();
  if (trimmed === '') return '';
  const idx = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'));
  const base = idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
  // 尾分隔符（`F:\dir\` 这类目录路径）取不到 basename：退回原串，便于用户识别
  return base === '' ? trimmed : base;
}

/** 路径列表 → 引用列表（去空串、按 path 去重，保留首次出现顺序）。 */
export function toFileRefs(paths: string[]): FileRef[] {
  const seen = new Set<string>();
  const refs: FileRef[] = [];
  for (const raw of paths) {
    const path = raw.trim();
    if (path === '' || seen.has(path)) continue;
    seen.add(path);
    refs.push({ path, name: basenameOf(path) });
  }
  return refs;
}

/** 引用列表 → 引用行文本（一行一个；空列表 → 空串）。 */
export function formatAttachmentLines(refs: FileRef[]): string {
  return refs.map((ref) => `@file:"${ref.path}"`).join('\n');
}

/**
 * 文本 → （正文, 引用列表）：把 `@file:"..."` 行提取成 refs，正文保留其余内容。
 *
 * 正文收尾规则：去掉因提取产生的尾部空行，最多保留一个尾换行（避免消息气泡
 * 底部多出一段空白）；提取后正文全为空则返回空串。**不含引用行时原文原样
 * 返回**（包括 `\r\n` 行尾）——普通聊天文本不走任何改写。
 */
export function parseAttachmentLines(text: string): { body: string; refs: FileRef[] } {
  if (!text.includes(ATTACHMENT_MARKER)) return { body: text, refs: [] };

  // 提取路径时统一行尾为 `\n`（与 MarkdownContent 的归一化口径一致）
  const lines = text.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n');
  const rest: string[] = [];
  const paths: string[] = [];
  for (const line of lines) {
    const matched = ATTACHMENT_LINE_RE.exec(line);
    if (matched) {
      paths.push(matched[1]);
    } else {
      rest.push(line);
    }
  }
  if (paths.length === 0) return { body: text, refs: [] };

  const joined = rest.join('\n');
  const body = joined.trim() === '' ? '' : joined.replace(/\n+$/, '\n');
  return { body, refs: toFileRefs(paths) };
}

/**
 * 正文 + 引用列表 → 最终消息文本：`正文` + 空行 + 引用行。
 *
 * 空行分隔是必须的：紧贴正文的 `@file:` 行会被 markdown 当成段内文本（或与
 * 上一行同段），模型侧按行解析时不易定位。**正文为空（纯附件消息）时不加
 * 前导空行**——消息气泡里不该出现一段无内容的空白。
 */
export function appendAttachments(content: string, refs: FileRef[]): string {
  if (refs.length === 0) return content;
  const body = content.replace(/\s+$/, '');
  const lines = formatAttachmentLines(refs);
  return body === '' ? lines : `${body}\n\n${lines}`;
}

/** 图标 + 颜色（chip 的视觉标识；图标来自 lucide，颜色用 Tailwind 语义色）。 */
export interface FileIconSpec {
  Icon: LucideIcon;
  className: string;
}

/** 代码类扩展名（源码 / 配置 / 脚本）。 */
const CODE_EXTS = new Set([
  'ts',
  'tsx',
  'js',
  'jsx',
  'mjs',
  'cjs',
  'py',
  'rs',
  'go',
  'java',
  'kt',
  'c',
  'h',
  'cpp',
  'hpp',
  'cc',
  'cs',
  'rb',
  'php',
  'swift',
  'json',
  'yaml',
  'yml',
  'toml',
  'xml',
  'html',
  'htm',
  'css',
  'scss',
  'less',
  'sh',
  'bash',
  'zsh',
  'ps1',
  'bat',
  'sql',
]);

/** 纯文本文档（pdf 在下方单独着色，不在此列）。 */
const DOC_EXTS = new Set(['md', 'markdown', 'txt', 'rst', 'log']);

/** 表格 / 数据表。 */
const SHEET_EXTS = new Set(['csv', 'tsv', 'xlsx', 'xls']);

/** 压缩包。 */
const ARCHIVE_EXTS = new Set(['zip', '7z', 'rar', 'tar', 'gz', 'tgz', 'bz2', 'xz']);

/**
 * 图片类扩展名。
 *
 * png/jpeg/webp/gif 正常走图片通道（data URL），只有它们以**路径引用**形式
 * 出现时（如桌面版拖了 png 但用户在图片通道之外添加）才用到这里的图标；
 * svg/bmp/ico/tiff 等则本来就不被图片通道接受（ADR-050 白名单），走引用通道
 * 时至少给个「这是图片」的视觉提示。
 */
const IMAGE_EXTS = new Set([
  'png',
  'jpg',
  'jpeg',
  'webp',
  'gif',
  'svg',
  'bmp',
  'ico',
  'tif',
  'tiff',
  'heic',
  'avif',
]);

/** 取小写扩展名（无扩展名 / 隐藏文件如 `.gitignore` → 空串）。 */
function extensionOf(name: string): string {
  const base = basenameOf(name);
  const dot = base.lastIndexOf('.');
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : '';
}

/** 按扩展名给图标与颜色（未知类型退回通用文件图标）。 */
export function fileIconFor(name: string): FileIconSpec {
  const ext = extensionOf(name);
  if (CODE_EXTS.has(ext)) return { Icon: FileCode, className: 'text-blue-500' };
  if (SHEET_EXTS.has(ext)) return { Icon: FileSpreadsheet, className: 'text-emerald-500' };
  if (ARCHIVE_EXTS.has(ext)) return { Icon: FileArchive, className: 'text-amber-500' };
  if (IMAGE_EXTS.has(ext)) return { Icon: FileImage, className: 'text-violet-500' };
  if (ext === 'pdf') return { Icon: FileText, className: 'text-red-500' };
  if (DOC_EXTS.has(ext)) return { Icon: FileText, className: 'text-[var(--color-text-secondary)]' };
  return { Icon: File, className: 'text-[var(--color-text-tertiary)]' };
}
