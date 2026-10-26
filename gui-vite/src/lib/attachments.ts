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

/**
 * 两种序列化格式（**读**都支持，写只有行内格式）：
 *
 * 1. **行内格式（当前唯一写入口径）**：`` `@file:"<绝对路径>"` `` —— 用 Markdown
 *    行内代码包裹，嵌在用户文字中间（`帮我看下 `@file:"…docx"` 的架构`）。文本
 *    里天然就是行内 code，消息区渲染时不会破坏段落/列表/表格的块结构，模型侧
 *    看到的也是一条连贯的句子而不是「正文 + 一堆路径行」。
 * 2. **整行格式（历史消息兼容，只读）**：`@file:"<绝对路径>"` 独占一行（批次 1
 *    的写法，消息末尾追加）。历史会话里存量消息是这个形状，消息区/回退仍要认。
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

/**
 * 文本 → （正文, 引用列表）：把 `@file:"..."` 行提取成 refs，正文保留其余内容。
 *
 * 正文收尾规则：去掉因提取产生的尾部空行，最多保留一个尾换行（避免消息气泡
 * 底部多出一段空白）；提取后正文全为空则返回空串。**不含引用行时原文原样
 * 返回**（包括 `\r\n` 行尾）——普通聊天文本不走任何改写。
 *
 * 只认**整行**旧格式（历史消息的兼容读路径）；新消息走行内格式，由消息区的
 * code 渲染器原位还原（见 `inlineChipPathOf`），不再进这里。
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
 * 一个文件引用 → 行内格式文本（**序列化唯一口径**，编辑器 chip 节点的
 * `getTextContent()` 就返回它）。
 *
 * 为什么是 Markdown 行内代码：模型看到的是 `` `@file:"C:\a b\c.ts"` ``，既直白
 * 可读、又不会被当成正文结构（路径里的 `*`/`_`/`[` 等 Markdown 元字符在 code
 * span 内全是字面量）。路径两侧的双引号用来界定边界——Windows 路径可含空格。
 */
export function serializeInlineChip(path: string): string {
  return '`@file:"' + path + '"`';
}

/** 行内 chip 的匹配口径：反引号包裹的 `@file:"<路径>"`（路径不含反引号与引号）。 */
const INLINE_CHIP_RE = /`@file:"([^`"]+)"`/g;

/** 行内 chip 的**单段**匹配口径：整段文本恰好是一个 chip 的序列化结果。 */
const INLINE_CHIP_ONLY_RE = /^@file:"([^`"]+)"$/;

/**
 * 单个行内代码的**文本内容**（已去掉反引号）→ 文件路径；不是 chip 则 `null`。
 *
 * 消息区的 code 渲染器每渲染一个 `<code>` 都要判一次（流式期间每帧都跑），
 * 所以这里只做一次锚定正则匹配，不做任何切分。
 */
export function inlineChipPathOf(code: string): string | null {
  const matched = INLINE_CHIP_ONLY_RE.exec(code);
  return matched === null ? null : matched[1];
}

/** 行内解析结果的一段：普通文字 / 一个文件引用。 */
export type InlineChipSegment = { type: 'text'; text: string } | { type: 'chip'; path: string };

/**
 * 文本 → 行内 segment 序列（编辑器的「外部 value 回填」用它重建 chip 节点）。
 *
 * 与 `parseAttachmentLines` 的口径不同：这里处理的是**行内**出现（chip 夹在
 * 文字中间），不能只匹配整行。两种格式都认：
 * - 行内格式 `` `@file:"…"` ``：任意位置，一行里可以有多个；
 * - 整行旧格式 `@file:"…"`：**独占一行**时也还原成 chip（回退旧消息时把历史
 *   文本塞回草稿，编辑器不该让用户看到一行裸路径）。
 *
 * 无 chip 时返回**单个文本段且内容原样**（含行尾差异）——普通文本不经过任何
 * 改写，这也让调用方可以安全地「先判有无再重建」。
 */
export function parseInlineChips(text: string): InlineChipSegment[] {
  if (!text.includes(ATTACHMENT_MARKER)) return [{ type: 'text', text }];

  // 提取时统一行尾为 `\n`（与编辑器内部的换行语义一致）
  const lines = text.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n');
  const segments: InlineChipSegment[] = [];
  let buffer = '';
  const flush = () => {
    if (buffer !== '') {
      segments.push({ type: 'text', text: buffer });
      buffer = '';
    }
  };

  for (let i = 0; i < lines.length; i += 1) {
    // 换行属于文字：先并入缓冲，chip 段之间的空行才不会被吞掉
    if (i > 0) buffer += '\n';
    const line = lines[i];
    const legacy = ATTACHMENT_LINE_RE.exec(line);
    if (legacy) {
      flush();
      segments.push({ type: 'chip', path: legacy[1] });
      continue;
    }
    let cursor = 0;
    INLINE_CHIP_RE.lastIndex = 0;
    for (let m = INLINE_CHIP_RE.exec(line); m !== null; m = INLINE_CHIP_RE.exec(line)) {
      buffer += line.slice(cursor, m.index);
      flush();
      segments.push({ type: 'chip', path: m[1] });
      cursor = m.index + m[0].length;
    }
    buffer += line.slice(cursor);
  }

  // 一个 chip 都没认出来（例如用户只打了半截 `@file:"`）→ 原文原样返回
  if (!segments.some((seg) => seg.type === 'chip')) return [{ type: 'text', text }];
  flush();
  return segments;
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
