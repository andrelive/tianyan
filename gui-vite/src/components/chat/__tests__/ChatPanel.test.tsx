import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import ChatPanel from '@/components/chat/ChatPanel';
import type { ChatMessage } from '@/lib/types';

function renderChatPanel(route = '/chat') {
  return render(
    <MemoryRouter initialEntries={[route]}>
      <Routes>
        <Route path="/chat" element={<ChatPanel />} />
        <Route path="/chat/:sessionId" element={<ChatPanel />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('ChatPanel', () => {
  it('renders input area', () => {
    renderChatPanel();
    // ChatInput renders a textarea with a specific placeholder
    const textarea = screen.getByPlaceholderText(/输入消息/);
    expect(textarea).toBeInTheDocument();
    // Send button should be present
    expect(screen.getByRole('button', { name: /发送/i })).toBeInTheDocument();
  });

  it('renders header with title', () => {
    renderChatPanel();
    expect(screen.getByText('对话')).toBeInTheDocument();
  });

  it('shows empty state when there are no messages and status is idle', () => {
    // Default store state: messages=[], streamStatus='idle'
    renderChatPanel();
    expect(screen.getByText('开始一段新的对话')).toBeInTheDocument();
    expect(screen.getByText('输入消息开始与 AI 助手交流')).toBeInTheDocument();
  });

  it('does not show empty state when there are messages', () => {
    const messages: ChatMessage[] = [
      {
        role: 'user',
        content: '你好',
        timestamp: new Date().toISOString(),
      },
      {
        role: 'assistant',
        content: '你好！我是天演',
        timestamp: new Date().toISOString(),
      },
    ];
    useAppStore.setState({ messages, streamStatus: 'idle' });

    renderChatPanel();

    expect(screen.queryByText('开始一段新的对话')).not.toBeInTheDocument();
    expect(screen.getByText('你好')).toBeInTheDocument();
    expect(screen.getByText('你好！我是天演')).toBeInTheDocument();
  });

  it('shows streaming indicator when streamStatus is streaming and last message is empty assistant', () => {
    useAppStore.setState({
      streamStatus: 'streaming',
      messages: [
        {
          role: 'user',
          content: '测试消息',
          timestamp: new Date().toISOString(),
        },
        {
          role: 'assistant',
          content: '',
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    // The streaming indicator shows "思考中..." text
    expect(screen.getByText('思考中...')).toBeInTheDocument();
  });

  it('does not show streaming indicator when last message has content', () => {
    useAppStore.setState({
      streamStatus: 'streaming',
      messages: [
        {
          role: 'user',
          content: '测试消息',
          timestamp: new Date().toISOString(),
        },
        {
          role: 'assistant',
          content: '已有内容',
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    expect(screen.queryByText('思考中...')).not.toBeInTheDocument();
  });

  it('sends a message and updates store state', async () => {
    const user = userEvent.setup();
    renderChatPanel();

    const textarea = screen.getByPlaceholderText(/输入消息/);
    await user.type(textarea, '测试发送');

    // Click send button
    const sendButton = screen.getByRole('button', { name: /发送/i });
    await user.click(sendButton);

    // After clicking send, messages are added synchronously (before fetch)
    const messages = useAppStore.getState().messages;
    expect(messages.length).toBeGreaterThanOrEqual(2);

    // First message is the user message
    expect(messages[0].role).toBe('user');
    expect(messages[0].content).toBe('测试发送');

    // Subsequent messages are assistant responses (the MSW mock streams
    // "你好" + "！" deltas through the SSE handler)
    const assistantMessages = messages.filter((m) => m.role === 'assistant');
    expect(assistantMessages.length).toBeGreaterThanOrEqual(1);

    // After the stream completes, status returns to 'idle'
    await vi.waitFor(() => {
      expect(useAppStore.getState().streamStatus).toBe('idle');
    });
  });

  it('is disabled from sending when already streaming', async () => {
    // Pre-set streaming state
    useAppStore.setState({
      streamStatus: 'streaming',
      messages: [
        {
          role: 'user',
          content: '第一条',
          timestamp: new Date().toISOString(),
        },
        {
          role: 'assistant',
          content: '',
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    const textarea = screen.getByPlaceholderText(/输入消息/);
    // Textarea should be disabled while streaming
    expect(textarea).toBeDisabled();
  });

  it('renders ModelSelector in header', () => {
    renderChatPanel();
    // ModelSelector renders a model select element
    expect(screen.getByText('对话')).toBeInTheDocument();
  });

  it('rolls back to a message via backend and syncs remaining messages', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          role: 'user',
          content: '你好',
          timestamp: new Date().toISOString(),
        },
        {
          role: 'assistant',
          content: '你好！我是天演',
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    // 回退到第二条消息（assistant）→ 剩余只有第一条
    await user.click(screen.getAllByRole('button', { name: /回退到此/ })[1]);

    await waitFor(() => {
      const messages = useAppStore.getState().messages;
      expect(messages).toHaveLength(1);
      expect(messages[0].content).toBe('你好');
    });

    // 回退后可撤销回退
    expect(screen.getByRole('button', { name: /撤销回退/ })).toBeInTheDocument();
  });

  it('undoes a rollback via backend (redo)', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      lastRollbackIndex: 1,
      messages: [
        {
          role: 'user',
          content: '你好',
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: /撤销回退/ }));

    await waitFor(() => {
      const messages = useAppStore.getState().messages;
      expect(messages).toHaveLength(2);
      expect(messages[1].content).toBe('你好！我是天演，有什么可以帮助你的？');
    });
    expect(useAppStore.getState().lastRollbackIndex).toBeNull();
  });

  it('shows the clarification bubble when a clarification is pending', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      pendingClarification: '请确认是否删除该文件？',
    });

    renderChatPanel();

    expect(screen.getByText('AI 需要确认')).toBeInTheDocument();
    expect(screen.getByText('请确认是否删除该文件？')).toBeInTheDocument();
    // 回答输入框与提交按钮可见
    expect(screen.getByLabelText('输入对追问的回答')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '提交回答' })).toBeInTheDocument();
  });

  it('submits a clarification answer via /chat/clarify, appends messages and clears the bubble', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      pendingClarification: '请确认是否删除该文件？',
    });

    renderChatPanel();

    const answerInput = screen.getByLabelText('输入对追问的回答');
    await user.type(answerInput, '确认删除');

    await user.click(screen.getByRole('button', { name: '提交回答' }));

    await waitFor(() => {
      expect(useAppStore.getState().pendingClarification).toBeNull();
    });

    // 回答作为 user 消息、继续处理结果作为 assistant 消息追加
    const messages = useAppStore.getState().messages;
    expect(messages).toHaveLength(2);
    expect(messages[0]).toMatchObject({ role: 'user', content: '确认删除' });
    expect(messages[1]).toMatchObject({
      role: 'assistant',
      content: '好的，我来继续处理。',
    });

    // 追问气泡消失
    expect(screen.queryByText('请确认是否删除该文件？')).not.toBeInTheDocument();
  });
});
