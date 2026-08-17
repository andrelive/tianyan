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
      <MessageBubble
        message={historyMsg}
        index={0}
        isStreaming={false}
        onRollback={() => {}}
      />,
    );

    // 工具卡片应出现
    expect(screen.getByText('读取文件')).toBeInTheDocument();
  });

  it('renders tool calls WITHOUT results when result is absent', () => {
    const streamMsg: ChatMessage = {
      role: 'assistant',
      content: '',
      segments: [{ type: 'tool', tool_call: { name: 'read_file', arguments: '{}', presentation: 'read' } }],
    };
    render(
      <MessageBubble
        message={streamMsg}
        index={0}
        isStreaming={false}
        onRollback={() => {}}
      />,
    );
    expect(screen.getByText('读取文件')).toBeInTheDocument();
  });
});