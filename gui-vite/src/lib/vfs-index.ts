/**
 * VFS 目录（L1）解析与渲染（与后端 `DocIndex` 结构对齐，ADR-001 修订 2026-09-22）。
 *
 * L1 语义 = **目录**：结构化 JSON（`{kind:"index",sections:[{title,summary,start_line,end_line}]}`）；
 * 非目录（短内容"直用"= 全文 / 旧概览 / 降级概览）为纯文本——`parseDocIndex` 返回 null，
 * 调用方按文本原样展示。
 */

export interface VfsIndexSection {
  title: string;
  summary: string;
  anchor?: string;
  start_line?: number;
  end_line?: number;
}

export interface VfsDocIndex {
  kind?: string;
  sections: VfsIndexSection[];
}

/** 解析 L1 目录 JSON；非目录 / 解析失败 / 无 sections → null。 */
export function parseDocIndex(text: string | null | undefined): VfsDocIndex | null {
  if (!text) return null;
  const trimmed = text.trim();
  if (!trimmed.startsWith('{')) return null;
  try {
    const parsed = JSON.parse(trimmed) as VfsDocIndex;
    if (!Array.isArray(parsed.sections) || parsed.sections.length === 0) return null;
    return parsed;
  } catch {
    return null;
  }
}

/** 渲染目录为可读列表（与后端 `render_doc_index` 格式对齐：有行号时随附）。 */
export function renderDocIndex(index: VfsDocIndex): string {
  return index.sections
    .map((s) => {
      if (s.start_line != null && s.end_line != null) {
        return `- ${s.title}（行 ${s.start_line}-${s.end_line}）：${s.summary}`;
      }
      return `- ${s.title}：${s.summary}`;
    })
    .join('\n');
}

/** 目录 → 渲染列表；非目录 → 原文本（空值原样返回空串）。 */
export function renderIndexOrText(text: string | null | undefined): string {
  if (!text) return text ?? '';
  const index = parseDocIndex(text);
  return index ? renderDocIndex(index) : text;
}
