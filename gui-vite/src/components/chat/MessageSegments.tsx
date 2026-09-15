import { memo, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import remarkGfm from 'remark-gfm';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { ChevronDown, ChevronRight } from 'lucide-react';
import type { MessageSegment, ToolCallWithResult } from '@/lib/types';
import { cn } from '@/lib/utils';
import ToolCallCard from './ToolCallCard';

/**
 * 消息段渲染共享组件（ADR-026：主对话流与子智能体流共用）。
 *
 * 从 MessageBubble 抽出：thinking → 折叠思考块、text → Markdown、
 * tool_call → ToolCallCard。子智能体消息流（面板展开区）复用同一套
 * 渲染，视觉与主对话流完全一致。
 */

/** 取文本的最后一行非空行（思考横幅用；流式期间即"当前思考"）。 */
function lastNonEmptyLine(text: string): string {
  const lines = text.split('\n');
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    const line = lines[i].trim();
    if (line !== '') return line;
  }
  return '';
}

/** 可折叠思考块（默认收起；收起时在 header 上以横幅形式显示思考输出的最后一行）。 */
export function ThinkingBlock({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const normalized = text.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
  const banner = lastNonEmptyLine(normalized);
  return (
    <div className="mb-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-tertiary)]/60 overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="w-full flex items-center gap-1.5 px-2.5 py-1.5 text-base text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)] transition-colors"
      >
        {open ? (
          <ChevronDown size={12} className="shrink-0" />
        ) : (
          <ChevronRight size={12} className="shrink-0" />
        )}
        <span className="shrink-0">思考过程</span>
        {/* 收起态横幅：在 header 上单行展示思考输出（最新一行，流式期间实时更新） */}
        {!open && banner !== '' && (
          <span className="flex-1 min-w-0 truncate pl-2 text-left italic opacity-70" title={banner}>
            {banner}
          </span>
        )}
      </button>
      {open && (
        <div className="px-3 pb-2 text-base leading-relaxed whitespace-pre-wrap text-[var(--color-text-secondary)] italic opacity-80 max-h-64 overflow-y-auto">
          {normalized}
        </div>
      )}
    </div>
  );
}

/** Markdown 正文渲染（共用样式与代码高亮）。memo：流式事件逐条到达时
 * 只有文本变化的调用方重解析 markdown——气泡因其他原因重渲染时跳过
 * 昂贵的解析/高亮（大会话流式卡死防护，与 ChatPanel onRollback 稳定化配套）。 */
export const MarkdownContent = memo(function MarkdownContent({
  text,
  isUser,
}: {
  text: string;
  isUser: boolean;
}) {
  return (
    <div
      className={cn(
        'text-lg leading-relaxed break-words',
        // U4：Markdown 软换行（段落内 \n）在浏览器默认 white-space 下折叠为空格——
        // 文本结构化内容（画线框图）与用户输入的多行文本换行丢失；pre-wrap 保留
        // 换行与连续空格（ASCII art 对齐），且仍允许长行自动换行（区别于 pre）。
        '[&_p]:whitespace-pre-wrap',
        '[&_li]:whitespace-pre-wrap',
        '[&_p]:mb-2 [&_p:last-child]:mb-0',
        '[&_ul]:mb-2 [&_ol]:mb-2',
        '[&_ul]:pl-5 [&_ol]:pl-5',
        '[&_ul]:list-disc [&_ol]:list-decimal',
        '[&_li]:mb-0.5',
        '[&_h1]:text-xl [&_h2]:text-lg',
        '[&_h1]:font-bold [&_h2]:font-semibold',
        '[&_blockquote]:border-l-2 [&_blockquote]:border-current',
        '[&_blockquote]:pl-3 [&_blockquote]:italic [&_blockquote]:opacity-80',
        '[&_a]:underline',
        isUser ? '[&_a]:text-blue-200' : '[&_a]:text-blue-500',
        '[&_hr]:border-[var(--color-border)] [&_hr]:my-2',
      )}
    >
      <ReactMarkdown
        // singleTilde: false —— 中文语境 15~20 是数值范围，单波浪线不应渲染为删除线
        remarkPlugins={[[remarkGfm, { singleTilde: false }]]}
        components={{
          table: ({ children }) => (
            <div className="my-2 overflow-x-auto">
              <table className="min-w-full border-collapse text-lg">{children}</table>
            </div>
          ),
          th: ({ children }) => (
            <th className="border border-[var(--color-border)] px-2 py-1 text-left font-semibold bg-[var(--color-bg-tertiary)]">
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td className="border border-[var(--color-border)] px-2 py-1">{children}</td>
          ),
          code: ({ className, children, ...props }: React.ComponentPropsWithoutRef<'code'>) => {
            const match = /language-(\w+)/.exec(className || '');
            const raw = String(children);
            const codeString = raw.replace(/\n$/, '');
            if (match) {
              return (
                <SyntaxHighlighter
                  style={oneDark}
                  language={match[1]}
                  PreTag="div"
                  customStyle={{
                    borderRadius: '8px',
                    fontSize: '0.8rem',
                    margin: '0.5rem 0',
                  }}
                >
                  {codeString}
                </SyntaxHighlighter>
              );
            }
            // U6：无语言标记的**围栏块**必须保留 pre 语义——ReactMarkdown 约定
            // 块级 code 的 children 以 \n 结尾（内联代码无尾换行）。pre 组件已
            // 透明化（避免与高亮器容器重复嵌套），故此处自行渲染块级容器：
            // 否则多行内容只剩 inline <code>（white-space: normal）→ 折叠成一行
            // （用户报告：目录树挤成一行）。
            if (raw.endsWith('\n')) {
              return (
                <pre className="my-2 overflow-x-auto rounded-lg bg-[var(--color-bg-tertiary)] p-3 text-[0.8rem] font-mono whitespace-pre">
                  <code>{codeString}</code>
                </pre>
              );
            }
            return (
              <code
                className={cn(
                  'px-1.5 py-0.5 rounded text-lg font-mono',
                  isUser ? 'bg-blue-600/30' : 'bg-[var(--color-bg-tertiary)]',
                )}
                {...props}
              >
                {children}
              </code>
            );
          },
          pre: ({ children }: React.ComponentPropsWithoutRef<'pre'>) => {
            return <>{children}</>;
          },
        }}
      >
        {/* 归一化行尾：模型输出可能带裸 \r（CRLF/孤立回车），统一转 \n 避免显示异常 */}
        {text.replace(/\r\n/g, '\n').replace(/\r/g, '\n')}
      </ReactMarkdown>
    </div>
  );
});

/** 时间线段渲染：按到达顺序轮番展示（相邻同类段合并，保持 markdown 连续）。
 * toolCalls：当前消息的工具调用列表（含 observation 挂载的结果）——
 * 流式 tool 段按 id 关联 result（applyToolResult 更新在 tool_calls 上，
 * 段自身不携带结果）。 */
export function SegmentBlocks({
  segments,
  toolCalls,
  isUser,
}: {
  segments: MessageSegment[];
  toolCalls?: ToolCallWithResult[];
  isUser: boolean;
}) {
  const blocks: React.ReactNode[] = [];
  let i = 0;
  let key = 0;
  while (i < segments.length) {
    const seg = segments[i];
    if (seg.type === 'thinking') {
      const texts: string[] = [];
      while (i < segments.length && segments[i].type === 'thinking') {
        const t = segments[i] as Extract<MessageSegment, { type: 'thinking' }>;
        texts.push(t.text);
        i++;
      }
      blocks.push(<ThinkingBlock key={key++} text={texts.join('')} />);
    } else if (seg.type === 'text') {
      const texts: string[] = [];
      while (i < segments.length && segments[i].type === 'text') {
        const t = segments[i] as Extract<MessageSegment, { type: 'text' }>;
        texts.push(t.text);
        i++;
      }
      blocks.push(<MarkdownContent key={key++} text={texts.join('')} isUser={isUser} />);
    } else {
      // 按调用 ID 关联结果（observation 已回填到 tool_calls；无 ID 回退名称+参数）
      const segCall = seg.tool_call;
      const merged: ToolCallWithResult = toolCalls?.find((c) => c.id === segCall.id) ??
        toolCalls?.find((c) => c.id === undefined && c.name === segCall.name) ?? {
          ...segCall,
          result: null,
        };
      blocks.push(<ToolCallCard key={key++} event={merged} result={merged.result} />);
      i++;
    }
  }
  return <>{blocks}</>;
}
