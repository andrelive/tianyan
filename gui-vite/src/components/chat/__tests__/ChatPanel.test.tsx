import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { resetTaskMocks, mockSessionCompressCalls } from '@/test/mocks/handlers';
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
  resetTaskMocks();
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
    expect(screen.getByText('会话')).toBeInTheDocument();
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

  it('handles server-side SSE error events: removes empty bubble and shows toast', async () => {
    const user = userEvent.setup();
    renderChatPanel();

    const textarea = screen.getByPlaceholderText(/输入消息/);
    // user.type 会把 [..] 当作键盘修饰符语法，改用 fireEvent.change
    fireEvent.change(textarea, { target: { value: '这是一个 [error-test] 请求' } });
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // 服务端返回 chunk_type=error 后：空气泡被清理、状态回到 idle、错误 toast 显示
    await vi.waitFor(() => {
      const state = useAppStore.getState();
      const emptyAssistant = state.messages.filter(
        (m) => m.role === 'assistant' && m.content === '',
      );
      expect(emptyAssistant).toHaveLength(0);
      expect(state.streamStatus).toBe('idle');
      expect(state.toast?.message).toContain('请求校验失败');
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
    expect(screen.getByText('会话')).toBeInTheDocument();
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

  it('disables the compress button when there is no session', () => {
    renderChatPanel();

    // 默认 store 无会话 → 按钮禁用
    expect(screen.getByRole('button', { name: '压缩会话' })).toBeDisabled();
  });

  it('compresses the current session and shows a success toast', async () => {
    const user = userEvent.setup();
    useAppStore.setState({ currentSessionId: 'session-1' });

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: '压缩会话' }));

    // compress API 被调用
    await waitFor(() => {
      expect(mockSessionCompressCalls).toEqual([{ sessionId: 'session-1' }]);
    });

    // 成功 toast（compressed=true → 已压缩）
    await waitFor(() => {
      expect(useAppStore.getState().toast?.message).toBe('已压缩');
    });
    expect(useAppStore.getState().toast?.type).toBe('success');
  });

  it('shows 无需压缩 when the backend reports nothing to compress', async () => {
    const user = userEvent.setup();
    useAppStore.setState({ currentSessionId: 'session-1' });
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return HttpResponse.json({ compressed: false });
      }),
    );

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: '压缩会话' }));

    await waitFor(() => {
      expect(useAppStore.getState().toast?.message).toBe('无需压缩');
    });
  });

  it('shows an error toast when compression fails', async () => {
    const user = userEvent.setup();
    useAppStore.setState({ currentSessionId: 'session-1' });
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: '压缩会话' }));

    await waitFor(() => {
      expect(useAppStore.getState().toast?.message).toContain('压缩失败');
    });
    expect(useAppStore.getState().toast?.type).toBe('error');
  });

  it('shows truncation hint when stream ends with finish_reason "length"', async () => {
    const user = userEvent.setup();
    renderChatPanel();

    const textarea = screen.getByPlaceholderText(/输入消息/);
    // user.type 会把 [..] 当作键盘修饰符语法，改用 fireEvent.change
    fireEvent.change(textarea, { target: { value: '这是一个 [length-test] 请求' } });
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // SSE 流结束事件 finish_reason='length' → 助手消息下方显示截断提示
    await waitFor(() => {
      expect(screen.getByText(/输出已达上限/)).toBeInTheDocument();
    });

    // 流结束后提示仍然保留
    await waitFor(() => {
      expect(useAppStore.getState().streamStatus).toBe('idle');
    });
    expect(screen.getByText(/输出已达上限/)).toBeInTheDocument();
  });

  it('does not show truncation hint when stream ends with finish_reason "stop"', async () => {
    const user = userEvent.setup();
    renderChatPanel();

    const textarea = screen.getByPlaceholderText(/输入消息/);
    await user.type(textarea, '普通请求');
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // 流正常结束后（finish_reason='stop'）不显示截断提示
    await waitFor(() => {
      expect(useAppStore.getState().streamStatus).toBe('idle');
    });
    expect(screen.queryByText(/输出已达上限/)).not.toBeInTheDocument();
  });

  it('accumulates thinking deltas separately and renders the collapsible block', async () => {
    const user = userEvent.setup();
    // 覆盖 SSE：thought chunk 携带 thinking 增量，answer chunk 携带正文
    server.use(
      http.post('/api/v1/chat/stream', () => {
        const encoder = new TextEncoder();
        const chunks = [
          'data: {"id":"msg-1","session_id":"session-1","delta":"","thinking":"先分析","chunk_type":"thought"}\n\n',
          'data: {"id":"msg-1","session_id":"session-1","delta":"","thinking":"再想想","chunk_type":"thought"}\n\n',
          'data: {"id":"msg-1","session_id":"session-1","delta":"最终输出","chunk_type":"answer"}\n\n',
          'data: {"id":"msg-1","session_id":"session-1","delta":"","finish_reason":"stop","chunk_type":"answer"}\n\n',
        ];
        const stream = new ReadableStream({
          start(controller) {
            for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
            controller.close();
          },
        });
        return new HttpResponse(stream, {
          headers: { 'Content-Type': 'text/event-stream' },
        });
      }),
    );
    renderChatPanel();

    const textarea = screen.getByPlaceholderText(/输入消息/);
    fireEvent.change(textarea, { target: { value: '带思考的问题' } });
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // 思考增量与正文分开累积（思考不入 content）
    await waitFor(() => {
      const assistant = useAppStore
        .getState()
        .messages.filter((m) => m.role === 'assistant');
      const last = assistant[assistant.length - 1];
      expect(last?.thinking).toBe('先分析再想想');
      expect(last?.content).toBe('最终输出');
    });

    // 思考块（可折叠）渲染在消息气泡中
    expect(await screen.findByRole('button', { name: /思考过程/ })).toBeInTheDocument();
    expect(screen.getByText('先分析再想想')).toBeInTheDocument();
  });
});
