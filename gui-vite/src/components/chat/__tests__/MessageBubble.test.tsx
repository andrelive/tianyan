import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import MessageBubble from '@/components/chat/MessageBubble';
import type { ChatMessage } from '@/lib/types';

describe('MessageBubble history rendering', () => {
  it('renders tool calls with results from history messages (no segments)', () => {
    const historyMsg: ChatMessage = {
      role: 'assistant',
      content: '已读取文件。',
      thinking: '思考过程',
      tool_calls: [
        {
          id: 'call_1',
          name: 'read_file',
          arguments: '{"path":"a.txt"}',
          presentation: 'read',
          result: '{"content":"文件内容abc"}',
        },
      ],
    };
    render(
      <MessageBubble message={historyMsg} index={0} isStreaming={false} onRollback={() => {}} />,
    );

    // 工具卡片应出现
    expect(screen.getByText('读取文件')).toBeInTheDocument();
  });

  it('renders tool calls WITHOUT results when result is absent', () => {
    const streamMsg: ChatMessage = {
      role: 'assistant',
      content: '',
      segments: [
        { type: 'tool', tool_call: { name: 'read_file', arguments: '{}', presentation: 'read' } },
      ],
    };
    render(
      <MessageBubble message={streamMsg} index={0} isStreaming={false} onRollback={() => {}} />,
    );
    expect(screen.getByText('读取文件')).toBeInTheDocument();
  });

  it('renders history messages with server segments via the timeline pipeline (方案 B)', () => {
    // 历史加载现在携带服务端权威 segments（由 StructuredMessage.parts 生成）：
    // 思考 → 工具前的正文 → 工具调用 → 工具后的正文——顺序与流式一致
    const historyMsg: ChatMessage = {
      role: 'assistant',
      content: '工具前的正文\n工具后的正文',
      thinking: '先想一步',
      tool_calls: [
        {
          id: 'call_1',
          name: 'read_file',
          arguments: '{"path":"a.txt"}',
          presentation: 'read',
          result: '{"content":"文件内容abc"}',
        },
      ],
      segments: [
        { type: 'thinking', text: '先想一步' },
        { type: 'text', text: '工具前的正文' },
        {
          type: 'tool',
          tool_call: {
            id: 'call_1',
            name: 'read_file',
            arguments: '{"path":"a.txt"}',
            presentation: 'read',
          },
        },
        { type: 'text', text: '工具后的正文' },
      ],
    };
    render(
      <MessageBubble message={historyMsg} index={0} isStreaming={false} onRollback={() => {}} />,
    );

    // 时间线渲染：思考块 + 工具卡片（带结果）+ 工具后正文
    expect(screen.getByRole('button', { name: /思考过程/ })).toBeInTheDocument();
    expect(screen.getByText('读取文件')).toBeInTheDocument();
    expect(screen.getByText('工具前的正文')).toBeInTheDocument();
    expect(screen.getByText('工具后的正文')).toBeInTheDocument();
  });
});
