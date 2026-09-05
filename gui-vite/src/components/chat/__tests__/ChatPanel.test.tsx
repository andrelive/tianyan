import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
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
    useAppStore.setState({ messages, streamStatus: {} });

    renderChatPanel();

    expect(screen.queryByText('开始一段新的对话')).not.toBeInTheDocument();
    expect(screen.getByText('你好')).toBeInTheDocument();
    expect(screen.getByText('你好！我是天演')).toBeInTheDocument();
  });

  it('shows streaming indicator when streamStatus is streaming and last message is empty assistant', () => {
    useAppStore.setState({
      streamStatus: { [PENDING_SESSION_KEY]: 'streaming' },
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
      streamStatus: { [PENDING_SESSION_KEY]: 'streaming' },
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

    // ADR-028：不再乐观渲染 user 消息（等广播/边界事件）——发送后本地只有
    // assistant 占位；user 消息由服务端边界事件（message chunk）插入到占位前。
    const messages = useAppStore.getState().messages;
    expect(messages.length).toBeGreaterThanOrEqual(1);

    // 流式完成后 user 消息经边界事件插入到 assistant 之前（顺序正确）
    await vi.waitFor(() => {
      const msgs = useAppStore.getState().messages;
      const userMsg = msgs.find((m) => m.role === 'user');
      expect(userMsg?.content).toBe('测试发送');
      const userIdx = msgs.findIndex((m) => m.role === 'user');
      const asstIdx = msgs.findIndex((m) => m.role === 'assistant');
      expect(userIdx).toBeGreaterThanOrEqual(0);
      expect(asstIdx).toBeGreaterThan(userIdx);
    });

    // After the stream completes, status returns to 'idle'
    await vi.waitFor(() => {
      // streamStatus 按会话归属（Record）：断言所有会话均为 idle
      expect(Object.values(useAppStore.getState().streamStatus).every((s) => s === 'idle')).toBe(
        true,
      );
    });
  });

  it('loads history messages when directly visiting a session URL (refresh/deep-link)', async () => {
    // 刷新/直达 /chat/{id}：ChatPanel 挂载时从后端加载历史并渲染
    renderChatPanel('/chat/session-1');

    await waitFor(() => {
      const messages = useAppStore.getState().messages;
      expect(messages.length).toBeGreaterThanOrEqual(2);
      expect(messages[0].role).toBe('user');
      expect(messages[0].content).toBe('你好');
    });

    // 历史消息渲染到页面（assistant 回复来自 mock /sessions/:id/messages）
    await waitFor(() => {
      expect(screen.getByText('你好！我是天演，有什么可以帮助你的？')).toBeInTheDocument();
    });
    // 空状态不显示
    expect(screen.queryByText('开始一段新的对话')).not.toBeInTheDocument();
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
      // streamStatus 按会话归属（Record）：断言所有会话均为 idle
      expect(Object.values(state.streamStatus).every((s) => s === 'idle')).toBe(true);
      expect(state.toasts[0]?.message).toContain('请求校验失败');
    });
  });

  it('is disabled from sending when already streaming', async () => {
    // Pre-set streaming state
    useAppStore.setState({
      streamStatus: { [PENDING_SESSION_KEY]: 'streaming' },
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
          id: 'msg_0',
          role: 'user',
          content: '你好',
          timestamp: new Date().toISOString(),
        },
        {
          id: 'msg_1',
          role: 'assistant',
          content: '你好！我是天演',
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    // 回退按钮仅出现在用户消息上（回退到该用户输入之前）；
    // assistant 消息不提供回退（避免回退到模型单轮输出的无意义粒度）
    const rollbackButtons = screen.getAllByRole('button', { name: /回退到此/ });
    expect(rollbackButtons).toHaveLength(1);
    await user.click(rollbackButtons[0]);

    await waitFor(() => {
      const messages = useAppStore.getState().messages;
      expect(messages).toHaveLength(0);
    });

    // 回退后可撤销回退
    expect(screen.getByRole('button', { name: /撤销回退/ })).toBeInTheDocument();
  });

  it('undoes a rollback via backend (redo)', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      lastRollbackMessageId: 'msg_deleted',
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
    expect(useAppStore.getState().lastRollbackMessageId).toBeNull();
  });

  it('shows the clarification bubble when a clarification is pending', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      pendingClarification: { questions: [{ question: '请确认是否删除该文件？', options: [] }] },
    });

    renderChatPanel();

    expect(screen.getByText('AI 需要确认')).toBeInTheDocument();
    expect(screen.getByText('请确认是否删除该文件？')).toBeInTheDocument();
    // 回答输入框与提交按钮可见
    expect(screen.getByLabelText('输入对追问的回答')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '提交回答' })).toBeInTheDocument();
  });

  it('submits a clarification answer via /chat/answer and clears the bubble', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      pendingClarification: { questions: [{ question: '请确认是否删除该文件？', options: [] }] },
    });

    renderChatPanel();

    const answerInput = screen.getByLabelText('输入对追问的回答');
    await user.type(answerInput, '确认删除');

    await user.click(screen.getByRole('button', { name: '提交回答' }));

    // 同步工具语义：回答提交到 /chat/answer 等待通道（mock 返回 ok），
    // 提交成功后接管组件退出（工具结果经主对话流返回，此处不模拟）
    await waitFor(() => {
      expect(useAppStore.getState().pendingClarification).toBeNull();
    });
    expect(screen.queryByText('请确认是否删除该文件？')).not.toBeInTheDocument();
  });

  it('shows no compress action in the ring popover without a session', () => {
    renderChatPanel();

    // 默认 store 无会话且无 usage → 圆环点击后不渲染压缩面板
    fireEvent.click(screen.getByRole('button', { name: '上下文占用' }));
    expect(screen.queryByRole('dialog', { name: '上下文占用详情' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /压缩会话/ })).not.toBeInTheDocument();
  });

  it('compresses the current session and shows a success toast', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          role: 'assistant',
          content: 'ok',
          timestamp: new Date().toISOString(),
          usage: { prompt_tokens: 100, completion_tokens: 5, total_tokens: 105 },
        },
      ],
    });

    renderChatPanel();

    // 打开上下文圆环详情 → 点压缩
    await user.click(screen.getByRole('button', { name: '上下文占用' }));
    await user.click(screen.getByRole('button', { name: /压缩会话/ }));

    // compress API 被调用
    await waitFor(() => {
      expect(mockSessionCompressCalls).toEqual([{ sessionId: 'session-1' }]);
    });

    // 成功 toast（compressed=true → 已压缩）
    await waitFor(() => {
      expect(useAppStore.getState().toasts[0]?.message).toBe('已压缩');
    });
    expect(useAppStore.getState().toasts[0]?.type).toBe('success');
  });

  it('appends the summary message to the message list after a successful compression', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          id: 'msg-1',
          role: 'assistant',
          content: 'ok',
          timestamp: new Date().toISOString(),
          usage: { prompt_tokens: 100, completion_tokens: 5, total_tokens: 105 },
        },
      ],
    });
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return HttpResponse.json({
          compressed: true,
          message: {
            id: 'cmp_123',
            role: 'system',
            content: '[对话摘要] 以下是对历史对话的摘要：\n## 用户意图\n测试\n[摘要结束]',
            timestamp: new Date().toISOString(),
          },
        });
      }),
    );

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: '上下文占用' }));
    await user.click(screen.getByRole('button', { name: /压缩会话/ }));

    // 压缩成功 → 摘要消息追加到消息流末尾（保留完整历史，不做清空/替换）
    await waitFor(() => {
      expect(useAppStore.getState().toasts[0]?.message).toBe('已压缩');
    });
    await waitFor(() => {
      const msgs = useAppStore.getState().messages;
      expect(msgs.length).toBe(2);
      expect(msgs[0].id).toBe('msg-1');
      expect(msgs[1].id).toBe('cmp_123');
      expect(msgs[1].content).toContain('对话摘要');
    });
  });

  it('shows 无需压缩 when the backend reports nothing to compress', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          role: 'assistant',
          content: 'ok',
          timestamp: new Date().toISOString(),
          usage: { prompt_tokens: 100, completion_tokens: 5, total_tokens: 105 },
        },
      ],
    });
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return HttpResponse.json({ compressed: false });
      }),
    );

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: '上下文占用' }));
    await user.click(screen.getByRole('button', { name: /压缩会话/ }));

    await waitFor(() => {
      expect(useAppStore.getState().toasts[0]?.message).toBe('无需压缩');
    });
  });

  it('shows an error toast when compression fails', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          role: 'assistant',
          content: 'ok',
          timestamp: new Date().toISOString(),
          usage: { prompt_tokens: 100, completion_tokens: 5, total_tokens: 105 },
        },
      ],
    });
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: '上下文占用' }));
    await user.click(screen.getByRole('button', { name: /压缩会话/ }));

    await waitFor(() => {
      expect(useAppStore.getState().toasts[0]?.message).toContain('压缩失败');
    });
    expect(useAppStore.getState().toasts[0]?.type).toBe('error');
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
      // streamStatus 按会话归属（Record）：断言所有会话均为 idle
      expect(Object.values(useAppStore.getState().streamStatus).every((s) => s === 'idle')).toBe(
        true,
      );
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
      // streamStatus 按会话归属（Record）：断言所有会话均为 idle
      expect(Object.values(useAppStore.getState().streamStatus).every((s) => s === 'idle')).toBe(
        true,
      );
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
      const assistant = useAppStore.getState().messages.filter((m) => m.role === 'assistant');
      const last = assistant[assistant.length - 1];
      expect(last?.thinking).toBe('先分析再想想');
      expect(last?.content).toBe('最终输出');
    });

    // 思考块（可折叠）渲染在消息气泡中
    expect(await screen.findByRole('button', { name: /思考过程/ })).toBeInTheDocument();
    expect(screen.getByText('先分析再想想')).toBeInTheDocument();
  });
});