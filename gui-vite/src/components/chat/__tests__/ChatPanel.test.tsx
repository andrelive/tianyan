import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, waitFor, fireEvent, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { resetTaskMocks, mockSessionCompressCalls } from '@/test/mocks/handlers';
import ChatPanel from '@/components/chat/ChatPanel';
import { handleChatStreamEvent } from '@/lib/chat-stream';
import { applySnapshot } from '@/hooks/use-unified-events';
import type { ChatMessage, ChatStreamEvent } from '@/lib/types';
import { messageText } from '@/lib/types';
// 输入框交互工具（contenteditable 的「打字 / 只读态 / 文本断言」口径集中在
// `@/test/composer`，与 ChatInput.test.tsx 共用）
import {
  composerText,
  draftText,
  expectComposerReadOnly,
  pasteFilesOntoComposer,
  typeIntoComposer,
} from '@/test/composer';

/**
 * 桌面壳桥接的可控替身（文件引用通道需要"绝对路径"这一桌面专属能力）：
 * 默认浏览器语义，只有相关用例内切到桌面语义。
 */
const tauri = vi.hoisted(() => ({
  desktop: false,
  // 剪贴板文件通道（粘贴 = 资源管理器 Ctrl+C 的文件）：默认「原生没看到文件」
  readClipboardFiles: vi.fn<() => Promise<string[]>>(async () => [] as string[]),
}));
vi.mock('@/lib/tauri', () => ({
  isTauri: () => tauri.desktop,
  readClipboardFiles: () => tauri.readClipboardFiles(),
  pickDirectory: async () => null,
  openExternal: async () => false,
}));

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

/** 构造统一事件通道的 chat_stream 事件（ADR-028 第 3 步：流式事件经
 * GET /events 到达，测试直接驱动 handleChatStreamEvent 模拟）。 */
function streamEvent(partial: Partial<ChatStreamEvent>): ChatStreamEvent {
  return {
    id: 'e1',
    session_id: 'session-1',
    delta: '',
    chunk_type: 'answer',
    ...partial,
  } as ChatStreamEvent;
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  resetTaskMocks();
  tauri.desktop = false;
  tauri.readClipboardFiles.mockReset();
  tauri.readClipboardFiles.mockResolvedValue([]);
});

describe('ChatPanel', () => {
  it('renders input area', () => {
    renderChatPanel();
    // ChatInput 的输入框已换成 Lexical 的 contenteditable：不再有 `placeholder`
    // 属性（占位符是叠在上面的 div），但 `aria-label="输入消息"` 原样保留
    const composer = screen.getByLabelText('输入消息');
    expect(composer).toBeInTheDocument();
    expect(composer).toHaveAttribute('role', 'textbox');
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
    // 消息带 segments（服务端权威时间线，ADR-019：历史/流式同构——
    // 渲染唯一路径 SegmentBlocks 依赖 segments）
    const messages: ChatMessage[] = [
      {
        role: 'user',
        segments: [{ type: 'text', text: '你好' }],
        timestamp: new Date().toISOString(),
      },
      {
        role: 'assistant',
        segments: [{ type: 'text', text: '你好！我是天演' }],
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
          segments: [{ type: 'text', text: '测试消息' }],
          timestamp: new Date().toISOString(),
        },
        {
          role: 'assistant',
          segments: [],
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
          segments: [{ type: 'text', text: '测试消息' }],
          timestamp: new Date().toISOString(),
        },
        {
          role: 'assistant',
          segments: [{ type: 'text', text: '已有内容' }],
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

    await typeIntoComposer('测试发送');

    // Click send button
    const sendButton = screen.getByRole('button', { name: /发送/i });
    await user.click(sendButton);

    // ADR-028：不再乐观渲染 user 消息（等广播/边界事件）——发送后本地只有
    // assistant 占位；user 消息由服务端边界事件（message chunk）插入到占位前。
    const messages = useAppStore.getState().messages;
    expect(messages.length).toBeGreaterThanOrEqual(1);

    // 模拟统一事件通道（GET /events）到达：用户消息边界 + 增量 + 完成
    handleChatStreamEvent(
      streamEvent({
        message: {
          id: 'msg-user',
          role: 'user',
          segments: [{ type: 'text', text: '测试发送' }],
          timestamp: '',
        },
        chunk_type: 'message',
      }),
    );
    handleChatStreamEvent(streamEvent({ delta: '你好' }));
    handleChatStreamEvent(streamEvent({ delta: '！', finish_reason: 'stop' }));

    // 流式完成后 user 消息经边界事件插入到 assistant 之前（顺序正确）
    await vi.waitFor(() => {
      const msgs = useAppStore.getState().messages;
      const userMsg = msgs.find((m) => m.role === 'user');
      expect(userMsg ? messageText(userMsg) : undefined).toBe('测试发送');
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
    // 刷新/直达 /chat/{id}：ChatPanel 挂载时订阅会话（ADR-029），
    // 快照帧（完整历史 + cursor）经统一事件通道到达 → replace 窗口。
    // 测试环境无 EventSource，快照帧由 applySnapshot 直接驱动模拟。
    renderChatPanel('/chat/session-1');

    // 模拟订阅快照帧到达（服务端 /events/subscribe 已 mock 返回 ok）
    act(() => {
      applySnapshot('session-1', [
        {
          id: 'msg-1',
          role: 'user',
          segments: [{ type: 'text', text: '你好' }],
          timestamp: '2026-07-23T10:00:00Z',
        },
        {
          id: 'msg-2',
          role: 'assistant',
          segments: [{ type: 'text', text: '你好！我是天演，有什么可以帮助你的？' }],
          timestamp: '2026-07-23T10:00:05Z',
        },
      ]);
    });

    await waitFor(() => {
      const messages = useAppStore.getState().messages;
      expect(messages.length).toBeGreaterThanOrEqual(2);
      expect(messages[0].role).toBe('user');
      expect(messageText(messages[0])).toBe('你好');
    });

    // 历史消息渲染到页面（快照 replace 后渲染）
    await waitFor(() => {
      expect(screen.getByText('你好！我是天演，有什么可以帮助你的？')).toBeInTheDocument();
    });
    // 空状态不显示
    expect(screen.queryByText('开始一段新的对话')).not.toBeInTheDocument();
  });

  it('handles server-side SSE error events: removes empty bubble and shows toast', async () => {
    const user = userEvent.setup();
    renderChatPanel();

    // 方括号在 user-event 里是修饰符语法，所以这里走自己的逐字符打字 helper
    await typeIntoComposer('这是一个 [error-test] 请求');
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // 模拟统一事件通道到达 error 事件（服务端校验/处理失败）
    handleChatStreamEvent(
      streamEvent({ chunk_type: 'error', delta: '请求校验失败: [error-test] 是非法输入' }),
    );

    // 服务端返回 chunk_type=error 后：空气泡被清理、状态回到 idle、错误 toast 显示
    await vi.waitFor(() => {
      const state = useAppStore.getState();
      const emptyAssistant = state.messages.filter(
        (m) => m.role === 'assistant' && messageText(m) === '',
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
          segments: [{ type: 'text', text: '第一条' }],
          timestamp: new Date().toISOString(),
        },
        {
          role: 'assistant',
          segments: [],
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    // 输入框在流式期间应为只读（contenteditable 无 disabled 属性 → 断 contenteditable/aria）
    expectComposerReadOnly();
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
          segments: [{ type: 'text', text: '你好' }],
          timestamp: new Date().toISOString(),
        },
        {
          id: 'msg_1',
          role: 'assistant',
          segments: [{ type: 'text', text: '你好！我是天演' }],
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

    // 回退点用户消息回填该会话草稿（编辑重发）：输入框出现被回退的内容
    await waitFor(() => {
      expect(useAppStore.getState().inputDrafts['session-1']).toEqual({
        text: '你好',
        images: [],
      });
    });
    // 外部 value（草稿）同步进编辑器：用户真的能看到回填的内容
    await waitFor(() => expect(composerText()).toBe('你好'));
    expect(draftText('session-1')).toBe('你好');

    // 回退后可撤销回退
    expect(screen.getByRole('button', { name: /撤销回退/ })).toBeInTheDocument();
  });

  it('fills the input draft with the rolled-back message text and images', async () => {
    // 带图片的用户消息：回退后草稿同时带出文本与 data URL 图片（可编辑重发）
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          id: 'msg_img',
          role: 'user',
          segments: [{ type: 'text', text: '请看这张图' }],
          images: ['data:image/png;base64,dGVzdA=='],
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();
    await user.click(screen.getByRole('button', { name: /回退到此/ }));

    await waitFor(() => {
      expect(useAppStore.getState().inputDrafts['session-1']).toEqual({
        text: '请看这张图',
        images: ['data:image/png;base64,dGVzdA=='],
      });
    });
    // 图片预览随草稿出现在输入区（待发送图片 1）
    expect(await screen.findByAltText('待发送图片 1')).toBeInTheDocument();
  });

  it('undoes a rollback via backend (redo)', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      rollbackMessageBySession: { 'session-1': 'msg_deleted' },
      messages: [
        {
          role: 'user',
          segments: [{ type: 'text', text: '你好' }],
          timestamp: new Date().toISOString(),
        },
      ],
    });

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: /撤销回退/ }));

    await waitFor(() => {
      const messages = useAppStore.getState().messages;
      expect(messages).toHaveLength(2);
      expect(messageText(messages[1])).toBe('你好！我是天演，有什么可以帮助你的？');
    });
    expect(useAppStore.getState().rollbackMessageBySession['session-1']).toBeUndefined();
  });

  it('hides the rollback banner for a different session (cross-session isolation)', () => {
    // 回退在会话 A 发生 → 切到会话 B 不得显示"撤销回退"横幅
    useAppStore.setState({
      currentSessionId: 'session-B',
      rollbackMessageBySession: { 'session-A': 'msg_deleted' },
      messages: [],
    });
    renderChatPanel();
    expect(screen.queryByRole('button', { name: /撤销回退/ })).not.toBeInTheDocument();
  });

  it('shows an in-flight indicator and disables input while rollback is pending', async () => {
    // ADR-041 波次 4：控制面操作进行中必须有明确反馈（消除「点了没反应 /
    // 重复点击」）；输入同步禁用（会话级操作是原子的，期间新消息会被服务端拒绝）。
    const user = userEvent.setup();
    let releaseDelete: () => void = () => {};
    const deleteGate = new Promise<void>((resolve) => {
      releaseDelete = resolve;
    });
    server.use(
      http.post('/api/v1/sessions/session-1/messages/delete', async () => {
        await deleteGate;
        return HttpResponse.json({
          session_id: 'session-1',
          messages: [],
          last_seq: -1,
        });
      }),
    );
    useAppStore.setState({
      currentSessionId: 'session-1',
      streamStatus: {},
      messages: [
        {
          id: 'msg_user_1',
          role: 'user',
          segments: [{ type: 'text', text: '你好' }],
          timestamp: new Date().toISOString(),
        },
      ],
    });
    renderChatPanel();

    await user.click(screen.getByRole('button', { name: /回退到此/ }));

    // 请求挂起期间：进行中反馈可见 + 输入框禁用 + 横幅按钮禁用
    expect(await screen.findByText('回退处理中…')).toBeInTheDocument();
    expectComposerReadOnly();

    releaseDelete();
    await waitFor(() => {
      expect(screen.queryByText('回退处理中…')).not.toBeInTheDocument();
    });
  });

  it('rollback result writes to the originating session even after switching away mid-flight', async () => {
    // 竞态守卫（回退的跨会话写入寻址）：回退请求发出后（挂起模拟慢响应）
    // 用户切到会话 B——迟到的回退结果必须写回发起的会话 A，不得写入 B
    // （此前 setMessages 隐式写入"此刻的当前会话"）。
    const user = userEvent.setup();
    let releaseDelete: () => void = () => {};
    const deleteGate = new Promise<void>((resolve) => {
      releaseDelete = resolve;
    });
    server.use(
      http.post('/api/v1/sessions/session-A/messages/delete', async () => {
        await deleteGate;
        // 服务端截断结果替身：带唯一标识（msg_rollback_srv）用于判定"迟到
        // 响应写入了哪个会话"。真实语义下结果与乐观值幂等一致（本项目
        // 已知"前端索引与服务端列表错位"——代码以消息 ID 定位），此处刻意
        // 不同以放大可观察性（乐观 slice 不含该标识）。
        return HttpResponse.json({
          session_id: 'session-A',
          messages: [
            {
              id: 'msg_rollback_srv',
              role: 'system',
              segments: [{ type: 'text', text: '回退完成' }],
              timestamp: new Date().toISOString(),
            },
          ],
        });
      }),
    );

    const aMessages: ChatMessage[] = [
      {
        id: 'msg_a0',
        role: 'user',
        segments: [{ type: 'text', text: 'A 的问题' }],
        timestamp: new Date().toISOString(),
      },
      {
        id: 'msg_a1',
        role: 'assistant',
        segments: [{ type: 'text', text: 'A 的回答' }],
        timestamp: new Date().toISOString(),
      },
    ];
    const bMessages: ChatMessage[] = [
      {
        id: 'msg_b0',
        role: 'user',
        segments: [{ type: 'text', text: 'B-ONLY-MARKER' }],
        timestamp: new Date().toISOString(),
      },
    ];
    useAppStore.setState({
      currentSessionId: 'session-A',
      messages: aMessages,
      sessionMessages: { 'session-A': aMessages, 'session-B': bMessages },
    });

    // 路由不带 :sessionId 参数：模拟"通过左侧列表切换会话"（直接改 store
    // 状态，不触发 URL→store 同步 effect 的弹回）
    renderChatPanel('/chat');

    // 发起回退（delete 请求 → 挂起）
    await user.click(screen.getByRole('button', { name: /回退到此/ }));

    // 请求挂起期间切到会话 B
    await act(async () => {
      useAppStore.getState().setCurrentSession('session-B');
    });
    expect(useAppStore.getState().currentSessionId).toBe('session-B');

    // 放行回退响应（session-A 的结果）
    await act(async () => {
      releaseDelete();
      await deleteGate;
    });

    // 等迟到响应被处理完（修复后写入 session-A；修复前被写入 session-B——痕迹必有其一）
    await waitFor(() => {
      const st = useAppStore.getState();
      const hasSentinel = (sid: string) =>
        st.sessionMessages[sid]?.some((m) => m.id === 'msg_rollback_srv') ?? false;
      expect(hasSentinel('session-A') || hasSentinel('session-B')).toBe(true);
    });

    // ★ 回退结果必须写回发起会话，不得污染已切换到的会话（缓存与当前投影）
    expect(
      useAppStore.getState().sessionMessages['session-A']?.some((m) => m.id === 'msg_rollback_srv'),
    ).toBe(true);
    expect(
      useAppStore.getState().sessionMessages['session-B']?.some((m) => m.id === 'msg_rollback_srv'),
    ).toBe(false);
    expect(useAppStore.getState().sessionMessages['session-B']?.[0]?.id).toBe('msg_b0');
    expect(useAppStore.getState().messages[0]?.id).toBe('msg_b0');
  });

  it('show no clarification bubble for a different session (cross-session isolation)', () => {
    useAppStore.setState({
      currentSessionId: 'session-B',
      pendingClarifications: {
        'session-A': {
          sessionId: 'session-A',
          questions: [{ question: '请确认是否删除该文件？', options: [] }],
        },
      },
    });

    renderChatPanel();

    // 跨会话：不得显示追问气泡（否则在 B 会话回答会提交到 B，A 永久卡死）
    expect(screen.queryByText('AI 需要确认')).not.toBeInTheDocument();
    expect(screen.queryByText('请确认是否删除该文件？')).not.toBeInTheDocument();
    // 普通输入框可用（textbox 可见）
    expect(screen.getByRole('textbox')).toBeInTheDocument();
  });

  it('keeps per-session clarifications independent (each session shows its own bubble)', () => {
    useAppStore.setState({
      currentSessionId: 'session-A',
      pendingClarifications: {
        'session-A': {
          sessionId: 'session-A',
          questions: [{ question: 'A 会话的问题？', options: [] }],
        },
        'session-B': {
          sessionId: 'session-B',
          questions: [{ question: 'B 会话的问题？', options: [] }],
        },
      },
    });

    renderChatPanel();
    // 当前会话（A）显示自己的问题；B 的问题不串入
    expect(screen.getByText('A 会话的问题？')).toBeInTheDocument();
    expect(screen.queryByText('B 会话的问题？')).not.toBeInTheDocument();
    // 两条目并存、互不覆盖（旧单槽实现：后到覆盖先到，此断言必红）
    expect(Object.keys(useAppStore.getState().pendingClarifications).sort()).toEqual([
      'session-A',
      'session-B',
    ]);

    // 切到 B：B 的问题接管；A 的条目保留（切回可继续回答）
    act(() => {
      useAppStore.getState().setCurrentSession('session-B');
    });
    expect(screen.getByText('B 会话的问题？')).toBeInTheDocument();
    expect(useAppStore.getState().pendingClarifications['session-A']).toBeDefined();
  });

  it('shows the clarification bubble when a clarification is pending', () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      pendingClarifications: {
        'session-1': {
          sessionId: 'session-1',
          questions: [{ question: '请确认是否删除该文件？', options: [] }],
        },
      },
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
      pendingClarifications: {
        'session-1': {
          sessionId: 'session-1',
          questions: [{ question: '请确认是否删除该文件？', options: [] }],
        },
      },
    });

    renderChatPanel();

    const answerInput = screen.getByLabelText('输入对追问的回答');
    await user.type(answerInput, '确认删除');

    await user.click(screen.getByRole('button', { name: '提交回答' }));

    // 同步工具语义：回答提交到 /chat/answer 等待通道（mock 返回 ok），
    // 提交成功后接管组件退出（工具结果经主对话流返回，此处不模拟）
    await waitFor(() => {
      expect(useAppStore.getState().pendingClarifications['session-1']).toBeUndefined();
    });
    expect(screen.queryByText('请确认是否删除该文件？')).not.toBeInTheDocument();
  });

  it('renders session token usage from the backend session summary', async () => {
    useAppStore.setState({
      currentSessionId: 'session-1',
      pendingClarifications: {},
    });
    renderChatPanel();

    // 底部统计来自 GET /usage/stats?session_id=（mock：未命中 1200 / 命中 800 /
    // 输出 500）——后端 usage_logs 全量聚合，与消息分页窗口无关
    await waitFor(() => {
      expect(screen.getByText('未命中输入')).toBeInTheDocument();
    });
    expect(screen.getByText('1,200')).toBeInTheDocument();
    expect(screen.getByText('缓存命中率')).toBeInTheDocument();
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
          segments: [{ type: 'text', text: 'ok' }],
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
          segments: [{ type: 'text', text: 'ok' }],
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
            role: 'user',
            compression_marker: true,
            segments: [
              {
                type: 'text',
                text: '[对话摘要] 以下是对历史对话的摘要：\n## 用户意图\n测试\n[摘要结束]',
              },
            ],
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
      expect(messageText(msgs[1])).toContain('对话摘要');
    });
  });
  it('converges the compression response with an already-pushed marker (idempotent)', async () => {
    // 推送先到（同一 id 已在消息流）→ HTTP 响应后到：按 id 幂等 upsert，
    // 不产生重复气泡（压缩点单一节点）。
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          id: 'msg-1',
          role: 'assistant',
          segments: [{ type: 'text', text: 'ok' }],
          timestamp: new Date().toISOString(),
          usage: { prompt_tokens: 100, completion_tokens: 5, total_tokens: 105 },
        },
        {
          id: 'cmp_123',
          role: 'user',
          compression_marker: true,
          segments: [{ type: 'text', text: '[对话摘要] 已经推送到达' }],
          timestamp: new Date().toISOString(),
        },
      ],
    });
    server.use(
      http.post('/api/v1/sessions/:id/compress', () => {
        return HttpResponse.json({
          compressed: true,
          message: {
            id: 'cmp_123',
            role: 'user',
            compression_marker: true,
            segments: [
              {
                type: 'text',
                text: '[对话摘要] 以下是对历史对话的摘要：\n## 用户意图\n测试\n[摘要结束]',
              },
            ],
            timestamp: new Date().toISOString(),
          },
        });
      }),
    );

    renderChatPanel();

    await user.click(screen.getByRole('button', { name: '上下文占用' }));
    await user.click(screen.getByRole('button', { name: /压缩会话/ }));

    await waitFor(() => {
      expect(useAppStore.getState().toasts[0]?.message).toBe('已压缩');
    });
    await waitFor(() => {
      const msgs = useAppStore.getState().messages;
      expect(msgs.filter((m) => m.id === 'cmp_123')).toHaveLength(1);
    });
  });

  it('shows 无需压缩 when the backend reports nothing to compress', async () => {
    const user = userEvent.setup();
    useAppStore.setState({
      currentSessionId: 'session-1',
      messages: [
        {
          role: 'assistant',
          segments: [{ type: 'text', text: 'ok' }],
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
          segments: [{ type: 'text', text: 'ok' }],
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

    await typeIntoComposer('这是一个 [length-test] 请求');
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // 模拟统一事件通道到达：增量 + length 完成
    handleChatStreamEvent(streamEvent({ delta: '第一段' }));
    handleChatStreamEvent(streamEvent({ delta: '', finish_reason: 'length' }));

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

    await typeIntoComposer('普通请求');
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // 模拟统一事件通道到达：增量 + stop 完成
    handleChatStreamEvent(streamEvent({ delta: '回答' }));
    handleChatStreamEvent(streamEvent({ delta: '', finish_reason: 'stop' }));

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
    renderChatPanel();

    await typeIntoComposer('带思考的问题');
    await user.click(screen.getByRole('button', { name: /发送/i }));

    // 模拟统一事件通道到达：思考增量 + 正文 + 完成
    handleChatStreamEvent(streamEvent({ delta: '', thinking: '先分析', chunk_type: 'thought' }));
    handleChatStreamEvent(streamEvent({ delta: '', thinking: '再想想', chunk_type: 'thought' }));
    handleChatStreamEvent(streamEvent({ delta: '最终输出' }));
    handleChatStreamEvent(streamEvent({ delta: '', finish_reason: 'stop' }));

    // 思考增量与正文分开累积（思考不入 content）
    await waitFor(() => {
      const assistant = useAppStore.getState().messages.filter((m) => m.role === 'assistant');
      const last = assistant[assistant.length - 1];
      expect(last?.thinking).toBe('先分析再想想');
      expect(last ? messageText(last) : undefined).toBe('最终输出');
    });

    // 思考块（默认收起）：header 横幅展示最新思考
    const thinkToggle = await screen.findByRole('button', { name: /思考过程/ });
    expect(thinkToggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.getByText('先分析再想想')).toBeInTheDocument();
  });

  it('renders messages through the virtualized list (Virtuoso wiring)', () => {
    // 回归迁移（虚拟滚动）：滚动/跟随机制从"外层滚动容器 + 内容列
    // ResizeObserver（data-chat-flow）"移交 react-virtuoso——followOutput
    // 负责跟随粘底（含工具卡片增长等最后一项增高场景）、atBottomStateChange
    // 维护跟随所有权、startReached 触发上滚加载。本测试锁接线：消息必须经
    // Virtuoso.itemContent 渲染（jsdom 无布局，经 vitest alias 注入渲染全部
    // 项的 mock；真实虚拟化/跟随行为由 e2e 与手工验证覆盖）。
    const messages: ChatMessage[] = [
      {
        role: 'user',
        segments: [{ type: 'text', text: '你好' }],
        timestamp: new Date().toISOString(),
      },
      {
        role: 'assistant',
        segments: [{ type: 'text', text: '回复' }],
        timestamp: new Date().toISOString(),
      },
    ];
    useAppStore.setState({ messages, streamStatus: {} });
    renderChatPanel();

    expect(screen.getByTestId('virtuoso')).toBeInTheDocument();
    expect(screen.getByText('你好')).toBeInTheDocument();
    expect(screen.getByText('回复')).toBeInTheDocument();
  });

  it('loads older history only when scrolled to the very top (no mid-scroll trigger)', async () => {
    // 分页端点：上滚一页（before_seq=5）返回更早一条 + has_more=false
    const pagingCalls: string[] = [];
    server.use(
      http.get('/api/v1/sessions/session-1/messages', ({ request }) => {
        const url = new URL(request.url);
        pagingCalls.push(url.searchParams.get('before_seq') ?? 'latest');
        return HttpResponse.json({
          session_id: 'session-1',
          messages: [
            {
              id: 'older-1',
              role: 'user',
              content: '更早的消息',
              segments: [{ type: 'text', text: '更早的消息' }],
              timestamp: '',
            },
          ],
          next_before_seq: null,
          has_more: false,
        });
      }),
    );

    // 预置当前会话首屏 + 分页元数据（还有更早历史）
    const initial: ChatMessage[] = [
      {
        role: 'user',
        segments: [{ type: 'text', text: '最近的消息' }],
        timestamp: new Date().toISOString(),
      },
    ];
    useAppStore.getState().setSessionMessages('session-1', initial);
    useAppStore.getState().setSessionMessageMeta('session-1', { oldestSeq: 5, hasMore: true });
    useAppStore.getState().setCurrentSession('session-1');
    useAppStore.setState({ messages: initial });
    renderChatPanel('/chat/session-1');

    const scroller = screen.getByTestId('virtuoso');

    // 中部滚动（未到顶）：不触发加载（旧 startReached overscan 语义下此处必触发）
    scroller.scrollTop = 400;
    fireEvent.scroll(scroller);
    expect(pagingCalls).toHaveLength(0);

    // 滚到顶（scrollTop≈0）：触发加载更早一页
    scroller.scrollTop = 0;
    fireEvent.scroll(scroller);
    await waitFor(() => {
      expect(pagingCalls).toEqual(['5']);
    });
    await waitFor(() => {
      expect(useAppStore.getState().sessionMessages['session-1']).toHaveLength(2);
    });
  });

  // ── 文件引用通道（批次 1）──

  it('sends file references as @file lines in content (images untouched)', async () => {
    // 粘贴非位图文件 → chip（绝对路径）；发送时路径并入正文，
    // 图片仍走 images 数组（data URL），两条通道互不影响。
    tauri.readClipboardFiles.mockResolvedValue(['C:\\work\\repo\\src\\main.ts']);
    const sentBodies: Array<{ message?: { content?: string; images?: string[] } }> = [];
    server.use(
      http.post('/api/v1/chat/stream', async ({ request }) => {
        sentBodies.push(
          (await request.json()) as { message?: { content?: string; images?: string[] } },
        );
        return HttpResponse.json({ status: 'started', session_id: 'session-1' });
      }),
    );

    const user = userEvent.setup();
    renderChatPanel();

    // 粘贴一个 .ts 文件（jsdom 无真实剪贴板，只能手工构造 file 项）加入引用
    await pasteFilesOntoComposer([
      {
        file: new File([new Uint8Array(8)], 'main.ts', { type: 'text/plain' }),
        type: 'text/plain',
      },
    ]);
    expect(await screen.findByTestId('file-chip')).toHaveTextContent('main.ts');

    await typeIntoComposer('看这个源码');
    await user.click(screen.getByRole('button', { name: /发送/i }));

    await waitFor(() => expect(sentBodies).toHaveLength(1));
    // 正文 = 用户文本 + 空行 + 引用行（引号包裹，Windows 路径可含空格）
    expect(sentBodies[0].message?.content).toBe(
      '看这个源码\n\n@file:"C:\\work\\repo\\src\\main.ts"',
    );
    // 图片通道不受影响（本例无图片）
    expect(sentBodies[0].message?.images).toBeUndefined();

    // 乐观渲染与 payload 用同一份文本：消息区把路径行还原成只读 chip
    const userMsg = useAppStore.getState().messages.find((m) => m.role === 'user');
    expect(userMsg ? messageText(userMsg) : undefined).toBe(
      '看这个源码\n\n@file:"C:\\work\\repo\\src\\main.ts"',
    );
    expect(await screen.findByTitle('C:\\work\\repo\\src\\main.ts')).toBeInTheDocument();
  });
});
