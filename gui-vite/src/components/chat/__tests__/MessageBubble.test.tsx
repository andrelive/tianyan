import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import MessageBubble from '@/components/chat/MessageBubble';
import type { ChatMessage } from '@/lib/types';

describe('MessageBubble interrupted hint', () => {
  it('shows the persistent interrupted hint (no interactive button)', () => {
    render(
      <MessageBubble
        message={{ role: 'assistant', content: '对了一半。本地快速通路：不是', interrupted: true }}
        index={0}
        isStreaming={false}
        onRollback={() => {}}
      />,
    );

    expect(screen.getByText('流式中断，已保留部分输出')).toBeInTheDocument();
    // 不渲染任何交互按钮（继续由用户在输入框自行发起）
    expect(screen.queryByRole('button', { name: '继续生成' })).not.toBeInTheDocument();
  });
});

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

  it('skips empty assistant messages from wake-turn empty output', () => {
    // 唤醒轮空输出（allow_empty_answer）持久化的空 assistant 消息：
    // 渲染层跳过，避免历史中出现只有时间戳的空白气泡。
    const emptyMsg: ChatMessage = {
      role: 'assistant',
      content: '',
      timestamp: new Date().toISOString(),
    };
    const { container } = render(
      <MessageBubble message={emptyMsg} index={0} isStreaming={false} onRollback={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it('still renders empty assistant message while streaming (placeholder)', () => {
    // 流式占位（content 为空但正在输出）必须保留，否则首字到达前无气泡
    const streamingMsg: ChatMessage = { role: 'assistant', content: '' };
    const { container } = render(
      <MessageBubble message={streamingMsg} index={0} isStreaming={true} onRollback={() => {}} />,
    );
    expect(container.firstChild).not.toBeNull();
  });
});
