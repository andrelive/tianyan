import { useRef, useEffect, useCallback, useState, useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import {
  compressSession,
  deleteSessionMessage,
  fetchApprovalStatus,
  fetchSessionMessages,
  getApiBase,
  redoSessionMessage,
  respondApproval,
} from '@/lib/api-client';
import type {
  ApprovalDecision,
  ApprovalStatusSnapshot,
  ChatMessage,
  ToolCallEvent,
} from '@/lib/types';
import { useChatStream } from '@/hooks/useChatStream';
import { usePolling } from '@/hooks/use-polling';
import { useSessionHistory } from '@/hooks/use-session-history';
import { toErrorMessage } from '@/lib/errors';
import { lastMessageUsage, sumSessionUsage } from '@/lib/token-usage';
import { MessageSquare, Loader2, Undo2 } from 'lucide-react';
import ChatInput from './ChatInput';
import ClarificationBubble from './ClarificationBubble';
import MessageBubble from './MessageBubble';
import { streamingIndicatorOwner } from './streaming-indicator';
import ApprovalBanner from './ApprovalBanner';
import { PENDING_SESSION_KEY } from '@/lib/store';

export default function ChatPanel() {
  const { sessionId: urlSessionId } = useParams<{ sessionId: string }>();

  // Store state - individual selectors for minimal re-renders
  const messages = useAppStore((s) => s.messages);
  const streamStatusMap = useAppStore((s) => s.streamStatus);
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  /** 当前会话流式状态（按会话归属：切到原会话的流继续，互不阻塞） */
  const streamStatus = streamStatusMap[currentSessionId ?? PENDING_SESSION_KEY] ?? 'idle';
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const addMessage = useAppStore((s) => s.addMessage);
  const setStreamStatus = useAppStore((s) => s.setStreamStatus);
  const deleteMessagesFrom = useAppStore((s) => s.deleteMessagesFrom);
  const lastRollbackMessageId = useAppStore((s) => s.lastRollbackMessageId);
  const setLastRollbackMessageId = useAppStore((s) => s.setLastRollbackMessageId);
  const pendingClarification = useAppStore((s) => s.pendingClarification);
  const setPendingClarification = useAppStore((s) => s.setPendingClarification);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [compressing, setCompressing] = useState(false);
  /** 唤醒轮停止标记（轮询失败/会话删除时置位；下一轮任务通知重新开始） */
  const [wakeStopped, setWakeStopped] = useState(false);
  /** 当前会话待审批操作（应用层授权卡片；wait_for_approval 模式挂起时出现） */
  const [pendingApproval, setPendingApproval] = useState<
    ApprovalStatusSnapshot['pending_approvals'][number] | null
  >(null);
  const [approvalBusy, setApprovalBusy] = useState(false);
  const selectedModel = useAppStore((s) => s.selectedModel);
  const chatModels = useAppStore((s) => s.chatModels);
  /** 最近一次流式完成 chunk 携带的真实窗口（会话流式时的权威值；
      历史会话回退到 chatModels[selectedModel].context_length）。 */
  const liveWindowRef = useRef(0);

  /** 当前会话自己的上下文占用（lib/token-usage 纯函数；切换会话随 messages 变化） */
  const lastUsage = useMemo(
    () =>
      lastMessageUsage(
        messages,
        chatModels.find((m) => m.name === selectedModel)?.context_length ?? liveWindowRef.current,
      ),
    [messages, chatModels, selectedModel],
  );

  /** 当前会话 token 消耗汇总（lib/token-usage 纯函数；DSH 风格小字展示） */
  const sessionUsage = useMemo(() => sumSessionUsage(messages), [messages]);

  // Sync URL sessionId to store on mount / navigation
  useEffect(() => {
    if (urlSessionId && urlSessionId !== currentSessionId) {
      setCurrentSession(urlSessionId);
      setPendingClarification(null);
    }
  }, [urlSessionId, currentSessionId, setCurrentSession, setPendingClarification]);

  // 历史加载（挂载恢复 + reloadSession）收敛在 use-session-history
  const { reloadSession } = useSessionHistory(urlSessionId);

  // ADR-013：唤醒轮自动刷新——会话末尾是后台任务 System 通知时轮询会话消息，
  // 直到出现新的非空 assistant 消息（主 agent 自动汇总结果，无需用户操作）。
  // 轮询语义收敛在 usePolling；enabled 派生自会话/流状态与任务通知。
  const lastMsg = messages[messages.length - 1];
  const hasTaskNotice = lastMsg?.role === 'system' && lastMsg.content.includes('后台任务');
  const wakeConditionsMet = !!currentSessionId && streamStatus !== 'streaming' && hasTaskNotice;
  // 条件重新满足（新一轮任务通知）时重置停止标记，恢复轮询
  useEffect(() => {
    if (wakeConditionsMet) setWakeStopped(false);
  }, [wakeConditionsMet]);
  usePolling(
    async () => {
      if (!currentSessionId) return;
      const data = await fetchSessionMessages(currentSessionId);
      const lastServer = data.messages[data.messages.length - 1];
      // 唤醒轮结果（非空 assistant）出现 → 刷新（hasTaskNotice 随 messages 消失，
      // enabled 自动翻 false 停止轮询）
      if (lastServer && lastServer.role === 'assistant' && lastServer.content !== '') {
        useAppStore.getState().setMessages(data.messages);
      }
    },
    3000,
    {
      enabled: wakeConditionsMet && !wakeStopped,
      // 轮询失败（会话删除等）：停止，避免无限重试
      onError: () => setWakeStopped(true),
    },
  );
  /** 唤醒轮指示（条件满足且未被停止） */
  const wakeActive = wakeConditionsMet && !wakeStopped;

  // 应用层授权：轮询审批状态，当前会话有挂起操作时显示审批卡片。
  // wait_for_approval 模式下危险操作由应用审批（与会话/LLM 无关），
  // 批准/拒绝后挂起的工具自动继续。轮询语义收敛在 usePolling。
  usePolling(
    async () => {
      if (!currentSessionId) return;
      const status = await fetchApprovalStatus();
      const mine = status.pending_approvals.find((p) => p.session_id === currentSessionId) ?? null;
      setPendingApproval((prev) => {
        if (prev?.request_id !== mine?.request_id) return mine;
        return prev;
      });
    },
    4000,
    { enabled: !!currentSessionId }, // 轮询失败静默（下次重试）
  );
  // 无会话时清空审批卡
  useEffect(() => {
    if (!currentSessionId) setPendingApproval(null);
  }, [currentSessionId]);

  const handleApproval = async (decision: ApprovalDecision) => {
    if (!pendingApproval || approvalBusy) return;
    setApprovalBusy(true);
    try {
      await respondApproval(pendingApproval.request_id, decision);
      setPendingApproval(null);
    } catch (err: unknown) {
      useAppStore.getState().showToast(`审批响应失败: ${toErrorMessage(err, '未知错误')}`, 'error');
    } finally {
      setApprovalBusy(false);
    }
  };

  // Auto-scroll to bottom when messages change
  useEffect(() => {
    if (scrollRef.current) {
      const el = scrollRef.current;
      // Only auto-scroll if user is near the bottom
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 150;
      if (isNearBottom) {
        requestAnimationFrame(() => {
          el.scrollTop = el.scrollHeight;
        });
      }
    }
  }, [messages, pendingClarification]);

  // 追问流与主对话流共用同一 hook 生命周期（fetch + 唯一 SSE 解析器 +
  // 事件归约器）；协议事件 → store 动作的唯一定义点在 lib/chat-stream
  const clarifyStream = useChatStream({
    streamUrl: `${getApiBase()}/chat/clarify/stream`,
    errorFallbackText: '追问回答失败',
    onComplete: (sessionId) => {
      // 流正常结束：接管已退出（提交时即清 pending），此处复位流状态；
      // 双保险清 pending（异常路径也会走到这里时无害）
      useAppStore.getState().setPendingClarification(null);
      useAppStore.getState().setStreamStatus('idle', sessionId);
    },
    onError: (sessionId, error) => {
      // 失败：接管退出 + 流状态复位 + 移除空占位 + 服务端消息同步
      // （本地消息 id 为 uuid，回退需服务端 msg_xxx 定位键）。
      // 此前缺失 pending 清理：气泡残留 + submitting 永远为真。
      useAppStore.getState().setPendingClarification(null);
      useAppStore.getState().setStreamStatus('idle', sessionId);
      useAppStore.getState().removeEmptyAssistantMessage();
      useAppStore.getState().showToast(`追问回答失败: ${error.message}`, 'error');
      if (sessionId) void reloadSession(sessionId);
    },
  });

  // ─── Custom streaming via fetch + ReadableStream ─────────────────

  // 流式事件归约在 lib/chat-stream（唯一解析器 + 协议→store 归约器）；
  // 本组件只提供流生命周期回调（错误重载、完成复位、usage 窗口记录）。
  const { startStream, stopStream } = useChatStream({
    streamUrl: `${getApiBase()}/chat/stream`,
    onUsageWindow: (windowTokens) => {
      liveWindowRef.current = windowTokens;
    },
    onError: (sessionId, error) => {
      // 流结束按归属会话置 idle（UI 可能已切走，不能用 currentSessionId 闭包值）
      useAppStore.getState().setStreamStatus('idle', sessionId);
      useAppStore.getState().showToast(`发送失败: ${error.message}`, 'error');
      // 同步服务端消息（本地消息 id 为 uuid，回退需服务端 msg_xxx 定位键；
      // 顺带清理失败残留的空 assistant 占位）
      if (sessionId) reloadSession(sessionId);
    },
    onComplete: (sessionId) => {
      useAppStore.getState().setStreamStatus('idle', sessionId);
    },
  });

  // 切走会话/界面后流继续在后台跑（fetch 循环持有归约器闭包，事件按
  // session_id 写入各自会话缓存）——不在此中止流，切回时直接显示累积内容。

  // ─── Handlers ───────────────────────────────────────────────────

  const handleSend = useCallback(
    async (content: string, images: string[] = []) => {
      if (streamStatus === 'streaming') return;
      const trimmed = content.trim();
      if (!trimmed && images.length === 0) return;

      // Add user message
      addMessage({
        role: 'user',
        content: trimmed,
        images: images.length > 0 ? images : undefined,
        timestamp: new Date().toISOString(),
      });

      // Add empty assistant placeholder for streaming
      addMessage({
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
      });

      // Build the API payload: only the new user message. The server's context
      // truth is the VFS-backed session history — sending the full local array
      // was dead weight and coupled the payload to the empty assistant
      // placeholder being the last element.
      const state = useAppStore.getState();
      const isNewSession = !state.currentSessionId;
      state.setStreamStatus('streaming');
      await startStream({
        session_id: state.currentSessionId,
        message: {
          role: 'user',
          content: trimmed,
          images: images.length > 0 ? images : undefined,
          timestamp: new Date().toISOString(),
        },
        stream: true,
        temperature: 0.7,
        max_tokens: 2048,
        model: state.selectedModel,
        // 会话级思考强度（对话时选择；off 不附加思考参数，仅对支持思考的模型生效）
        thinking: state.thinkingEffort === 'off' ? undefined : state.thinkingEffort,
        // 新会话绑定工作区（工作区 = 会话的父级分组；服务端固化到会话头部）
        working_directory: isNewSession ? (state.newSessionWorkspace ?? undefined) : undefined,
      });
      // 新会话已创建并固化工作区绑定：清除待绑定状态（每个新对话重新选择）
      if (isNewSession) {
        useAppStore.getState().setNewSessionWorkspace(null);
      }
    },
    [streamStatus, addMessage, startStream],
  );

  const handleRollback = useCallback(
    async (index: number) => {
      if (streamStatus === 'streaming') return;

      const state = useAppStore.getState();
      const sessionId = state.currentSessionId;
      if (!sessionId) {
        state.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }

      // 被回退消息的 ID：重做数据的定位键（前端索引与服务端列表错位，
      // 数字索引不可靠——按 ID 定位删除）
      const target = messages[index];
      if (!target?.id) {
        state.showToast('该消息缺少 ID，无法回退', 'error');
        return;
      }

      // 乐观更新：回退到该消息之前（删除该消息及其后）
      deleteMessagesFrom(index);
      try {
        const resp = await deleteSessionMessage(sessionId, target.id);
        state.setMessages(resp.messages);
        setLastRollbackMessageId(target.id);
      } catch (err: unknown) {
        state.showToast(`回退失败: ${toErrorMessage(err, '未知错误')}`, 'error');
        await reloadSession(sessionId);
      }
    },
    [streamStatus, deleteMessagesFrom, setLastRollbackMessageId, messages, reloadSession],
  );

  // 撤销回滚：恢复被删除的消息与工作区文件
  const handleRedo = useCallback(async () => {
    if (streamStatus === 'streaming' || lastRollbackMessageId === null) return;

    const state = useAppStore.getState();
    const sessionId = state.currentSessionId;
    if (!sessionId) return;

    try {
      const resp = await redoSessionMessage(sessionId, lastRollbackMessageId);
      state.setMessages(resp.messages);
      setLastRollbackMessageId(null);
      state.showToast('已撤销回退', 'success');
    } catch (err: unknown) {
      state.showToast(`撤销回退失败: ${toErrorMessage(err, '未知错误')}`, 'error');
      await reloadSession(sessionId);
    }
  }, [streamStatus, lastRollbackMessageId, setLastRollbackMessageId, reloadSession]);

  // 提交对 Agent 追问的回答（流式）：确认后思考/工具/输出逐块渲染，
  // 避免整轮等待超过 HTTP 超时（此前非流式路径表现为"按钮转圈后报错"）。
  // 生命周期交给 clarifyStream hook（fetch/consume/错误语义与主对话流一致）
  const handleClarify = useCallback(
    async (answer: string) => {
      if (streamStatus === 'streaming' || clarifyStream.isStreaming) return;
      const state = useAppStore.getState();
      const sessionId = state.currentSessionId;
      if (!sessionId) {
        state.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }
      // 回答作为 ask_user 工具结果挂到工具卡片（工具链语义：回答是
      // 工具的输入，不是新一轮用户输入——对齐 DSH，不进消息流）
      const askCall = findAskUserCall(useAppStore.getState().messages);
      if (askCall && askCall.id) {
        useAppStore.getState().applyToolResult({
          tool_call_id: askCall.id,
          content: answer,
          success: true,
          duration_ms: 0,
        });
      }
      // 助手占位消息：澄清轮流式增量累积其上
      addMessage({
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
      });
      // 回答已受理：接管组件立即退出（对齐 DSH question/resolved 移除
      // composer），普通输入框恢复；流式期间锁定输入（防并发轮）
      state.setPendingClarification(null);
      state.setStreamStatus('streaming');
      await clarifyStream.startStream({ session_id: sessionId, answer });
    },
    [streamStatus, addMessage, clarifyStream],
  );

  const handleStop = useCallback(() => {
    stopStream();
    setStreamStatus('idle');
  }, [stopStream, setStreamStatus]);

  // 手动压缩当前会话上下文
  const handleCompress = useCallback(async () => {
    const state = useAppStore.getState();
    const sessionId = state.currentSessionId;
    if (!sessionId || streamStatus === 'streaming' || compressing) return;
    setCompressing(true);
    try {
      const resp = await compressSession(sessionId);
      state.showToast(resp.compressed ? '已压缩' : '无需压缩', 'success');
    } catch (err: unknown) {
      state.showToast(`压缩失败: ${toErrorMessage(err, '未知错误')}`, 'error');
    } finally {
      setCompressing(false);
    }
  }, [streamStatus, compressing]);

  // Determine which message is currently streaming
  const streamingIndex = streamStatus === 'streaming' ? messages.length - 1 : -1;

  // ─── Render ────────────────────────────────────────────────────

  return (
    <div className="flex flex-col h-full bg-[var(--color-bg-primary)]">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] shrink-0">
        <h1 className="text-lg font-semibold text-[var(--color-text-primary)]">会话</h1>
        <div className="flex items-center gap-2" />
      </div>

      {/* Messages area */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto px-4 py-4">
        {messages.length === 0 && streamStatus === 'idle' && (
          <div className="flex flex-col items-center justify-center h-full text-[var(--color-text-tertiary)] gap-3">
            <MessageSquare className="w-12 h-12 opacity-30" />
            <p className="text-sm">开始一段新的对话</p>
            <p className="text-xs opacity-60">输入消息开始与 AI 助手交流</p>
          </div>
        )}

        {messages.length > 0 && (
          <div className="space-y-4 max-w-4xl mx-auto">
            {messages.map((msg, i) => (
              <MessageBubble
                key={msg.id || `msg-${i}`}
                message={msg}
                index={i}
                isStreaming={i === streamingIndex && msg.role === 'assistant'}
                onRollback={handleRollback}
              />
            ))}

            {/* Loading indicator: streaming started but no content yet.
                归属判定单点（streamingIndicatorOwner）：thinking 非空时由气泡内指示接管。 */}
            {streamStatus === 'streaming' &&
              messages.length > 0 &&
              messages[messages.length - 1].role === 'assistant' &&
              streamingIndicatorOwner(messages[messages.length - 1]) === 'list' && (
                <div
                  className="flex items-center gap-2 text-[var(--color-text-tertiary)] py-2"
                  aria-live="polite"
                  aria-label="AI 正在思考中"
                >
                  <Loader2 className="w-4 h-4 animate-spin" />
                  <span className="text-sm">思考中...</span>
                </div>
              )}

            {/* Wake polling indicator: 后台任务完成后主 agent 正在自动汇总 */}
            {wakeActive && streamStatus === 'idle' && (
              <div
                className="flex items-center gap-2 text-[var(--color-text-tertiary)] py-2"
                aria-live="polite"
                aria-label="等待后台任务汇总"
              >
                <Loader2 className="w-4 h-4 animate-spin" />
                <span className="text-sm">后台任务完成，AI 正在汇总结果...</span>
              </div>
            )}
          </div>
        )}

        {/* Redo banner: 回退后可撤销 */}
        {lastRollbackMessageId !== null && streamStatus !== 'streaming' && (
          <div className="flex justify-center pb-1">
            <button
              onClick={handleRedo}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-full border border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors"
              aria-label="撤销回退"
            >
              <Undo2 className="w-3.5 h-3.5" />
              已回退 — 撤销回退
            </button>
          </div>
        )}
      </div>

      {/* 应用层授权卡片（提取自 ChatPanel 内联；与 ApprovalPanel 共用语义） */}
      <ApprovalBanner
        approval={pendingApproval}
        busy={approvalBusy}
        onRespond={(decision) => void handleApproval(decision)}
      />

      {/* Input area（模型/思考强度/上下文圆环 + 发送：DSH 布局）
          追问待回答时由 ClarificationBubble 接管（composer takeover，对齐 DSH）：
          输入框区域被问题表单替代，回答提交后恢复 */}
      {pendingClarification && streamStatus !== 'streaming' ? (
        <ClarificationBubble
          question={pendingClarification.question}
          options={pendingClarification.options}
          submitting={clarifyStream.isStreaming}
          onSubmit={handleClarify}
        />
      ) : (
        <ChatInput
          onSend={handleSend}
          onStop={handleStop}
          isStreaming={streamStatus === 'streaming'}
          usage={lastUsage}
          sessionUsage={sessionUsage}
          onCompress={() => void handleCompress()}
        />
      )}
    </div>
  );
}

/** 在消息流中定位 ask_user 工具调用（回答提交时挂工具结果用）。 */
function findAskUserCall(messages: ChatMessage[]): ToolCallEvent | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role === 'assistant' && m.tool_calls && m.tool_calls.length > 0) {
      const call = m.tool_calls.find((c) => c.name === 'ask_user');
      if (call) return call;
    }
  }
  return null;
}
