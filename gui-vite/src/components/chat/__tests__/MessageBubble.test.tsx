import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import MessageBubble from '@/components/chat/MessageBubble';
import type { ChatMessage } from '@/lib/types';

describe('MessageBubble interrupted hint', () => {
  it('shows the persistent interrupted hint (no interactive button)', () => {
    render(
      <MessageBubble
        message={{ role: 'assistant', segments: [{ type: 'text', text: '对了一半。本地快速通路：不是' }], interrupted: true }}
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
  it('renders user message content without segments (regression: 空气泡)', () => {
    // 服务端 segments 只属于 assistant（segments_from_parts 对非 assistant
    // 恒返回 None）——用户消息直接渲染 content，不依赖 segments
    const userMsg: ChatMessage = {
      role: 'user',
      segments: [{ type: 'text', text: '请写一篇 800 字的文章' }],
      timestamp: '2026-09-06T00:00:00Z',
    };
    render(
      <MessageBubble message={userMsg} index={0} isStreaming={false} onRollback={() => {}} />,
    );
    expect(screen.getByText('请写一篇 800 字的文章')).toBeInTheDocument();
  });

  it('renders tool calls with results from history messages', () => {
    // 历史消息携带服务端权威 segments（ADR-019：由 StructuredMessage.parts
    // 生成，tool 段 + 结果挂 tool_calls）——渲染唯一路径 SegmentBlocks
    const historyMsg: ChatMessage = {
      role: 'assistant',
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
      segments: [
        { type: 'thinking', text: '思考过程' },
        {
          type: 'tool',
          tool_call: {
            id: 'call_1',
            name: 'read_file',
            arguments: '{"path":"a.txt"}',
            presentation: 'read',
          },
        },
        { type: 'text', text: '已读取文件。' },
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
      segments: [],
      timestamp: new Date().toISOString(),
    };
    const { container } = render(
      <MessageBubble message={emptyMsg} index={0} isStreaming={false} onRollback={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it('still renders empty assistant message while streaming (placeholder)', () => {
    // 流式占位（content 为空但正在输出）必须保留，否则首字到达前无气泡
    const streamingMsg: ChatMessage = { role: 'assistant', segments: [] };
    const { container } = render(
      <MessageBubble message={streamingMsg} index={0} isStreaming={true} onRollback={() => {}} />,
    );
    expect(container.firstChild).not.toBeNull();
  });
});

describe('MessageBubble compression marker', () => {
  it('renders the 压缩点 badge for compression summary messages', () => {
    // 压缩点标识（compression_marker）：实时推送后在页面上可辨识——徽章
    // 渲染于消息顶部，正文仍走统一时间线（SegmentBlocks）。压缩点为 user
    // 锚定（模型侧角色），展示侧保持无框节点：不渲染用户气泡/回退按钮。
    const summaryMsg: ChatMessage = {
      role: 'user',
      compression_marker: true,
      segments: [{ type: 'text', text: '[对话摘要] 以下是对历史对话的摘要：……' }],
    };
    render(
      <MessageBubble message={summaryMsg} index={1} isStreaming={false} onRollback={() => {}} />,
    );

    expect(screen.getByText('压缩点')).toBeInTheDocument();
    // 非用户输入：不提供回退按钮（回退锚点只属于真实用户输入）
    expect(screen.queryByLabelText('回退到此')).not.toBeInTheDocument();
    expect(screen.getByText(/对话摘要/)).toBeInTheDocument();
  });

  it('renders the badge for legacy (system-role) compression summaries too', () => {
    // 旧数据兼容：历史库里摘要持久化为 system 角色——渲染按 marker 判定，不依赖角色。
    const legacyMsg: ChatMessage = {
      role: 'system',
      compression_marker: true,
      segments: [{ type: 'text', text: '[对话摘要] 旧数据摘要' }],
    };
    render(
      <MessageBubble message={legacyMsg} index={1} isStreaming={false} onRollback={() => {}} />,
    );
    expect(screen.getByText('压缩点')).toBeInTheDocument();
  });

  it('does not render the badge for regular system notifications', () => {
    const notifyMsg: ChatMessage = {
      role: 'system',
      segments: [{ type: 'text', text: '[后台命令完成] cargo tauri build（cmd_0）' }],
    };
    render(
      <MessageBubble message={notifyMsg} index={0} isStreaming={false} onRollback={() => {}} />,
    );

    expect(screen.queryByText('压缩点')).not.toBeInTheDocument();
    expect(screen.getByText(/后台命令完成/)).toBeInTheDocument();
  });
});

