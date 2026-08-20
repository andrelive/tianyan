import { useRef, useEffect, useCallback, useState, useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import {
  apiGet,
  compressSession,
  deleteSessionMessage,
  fetchApprovalStatus,
  getApiBase,
  redoSessionMessage,
  respondApproval,
} from '@/lib/api-client';
import { ShieldAlert, Check, X } from 'lucide-react';
import type { ApprovalDecision, ApprovalStatusSnapshot } from '@/lib/types';
import { useChatStream } from '@/hooks/useChatStream';
import { MessageSquare, Loader2, Undo2 } from 'lucide-react';
import ChatInput from './ChatInput';
import ClarificationBubble from './ClarificationBubble';
import MessageBubble from './MessageBubble';
import type { ChatMessage, ChatStreamEvent, StreamUsage } from '@/lib/types';

export default function ChatPanel() {
  const { sessionId: urlSessionId } = useParams<{ sessionId: string }>();

  // Store state - individual selectors for minimal re-renders
  const messages = useAppStore((s) => s.messages);
  const streamStatus = useAppStore((s) => s.streamStatus);
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const addMessage = useAppStore((s) => s.addMessage);
  const setStreamStatus = useAppStore((s) => s.setStreamStatus);
  const deleteMessagesFrom = useAppStore((s) => s.deleteMessagesFrom);
  const setMessages = useAppStore((s) => s.setMessages);
  const lastRollbackIndex = useAppStore((s) => s.lastRollbackIndex);
  const setLastRollbackIndex = useAppStore((s) => s.setLastRollbackIndex);
  const pendingClarification = useAppStore((s) => s.pendingClarification);
  const setPendingClarification = useAppStore((s) => s.setPendingClarification);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [submittingClarify, setSubmittingClarify] = useState(false);
  const [compressing, setCompressing] = useState(false);
  const [wakePolling, setWakePolling] = useState(false);
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
  /** 实时当前会话 ID（ref 镜像：SSE chunk 回调里读取，避免闭包过期）。
      用于会话守卫——切换会话/界面后到达的残留 chunk 直接丢弃，
      防污染新会话的消息列表。 */
  const sessionIdRef = useRef<string | null>(null);

  /** 当前会话自己的上下文占用：取本会话最后一条带 usage 的消息（消息级独立计算，
      切换会话随 messages 变化——每个会话显示各自的占用，不再串值）。 */
  const lastUsage = useMemo<StreamUsage | null>(() => {
    const modelWindow =
      chatModels.find((m) => m.name === selectedModel)?.context_length ?? liveWindowRef.current;
    for (let i = messages.length - 1; i >= 0; i--) {
      const u = messages[i].usage;
      if (u && u.prompt_tokens > 0) {
        return {
          prompt_tokens: u.prompt_tokens,
          completion_tokens: u.completion_tokens,
          total_tokens: u.total_tokens,
          cache_read: u.cache_read ?? 0,
          cache_write: u.cache_write ?? 0,
          context_window: modelWindow || 0,
        };
      }
    }
    return null;
  }, [messages, chatModels, selectedModel]);

  /** 当前会话 token 消耗汇总（跨全部消息 usage 累加；DSH 风格小字展示：
   缓存未命中输入 / 缓存命中输入 / 输出 / 缓存命中率）。 */
  const sessionUsage = useMemo(() => {
    let uncachedInput = 0;
    let cachedInput = 0;
    let completion = 0;
    for (const m of messages) {
      const u = m.usage;
      if (!u || u.prompt_tokens <= 0) continue;
      const cached = u.cache_read ?? 0;
      cachedInput += cached;
      uncachedInput += Math.max(0, u.prompt_tokens - cached);
      completion += u.completion_tokens;
    }
    if (uncachedInput + cachedInput + completion === 0) return null;
    return { uncachedInput, cachedInput, completion };
  }, [messages]);

  // Sync URL sessionId to store on mount / navigation
  useEffect(() => {
    if (urlSessionId && urlSessionId !== currentSessionId) {
      setCurrentSession(urlSessionId);
      setMessages([]);
      setPendingClarification(null);
    }
  }, [urlSessionId, currentSessionId, setCurrentSession, setMessages, setPendingClarification]);

  // 刷新/直达 URL（如 /chat/{id}）时恢复历史消息：仅在挂载时执行一次。
  // 列表点击路径由 SessionList.handleSelectSession 负责加载（导航后
  // currentSessionId 已一致，此处不会重复请求）。
  // StrictMode 双跑安全：ref 守卫只允许一次；cleanup 不取消请求（取消会
  // 杀死唯一请求），改由「应用前校验当前 URL 仍指向该会话」防旧请求覆盖。
  const loadedOnMountRef = useRef(false);
  const activeUrlSessionRef = useRef<string | null>(null);
  useEffect(() => {
    activeUrlSessionRef.current = urlSessionId ?? null;
  }, [urlSessionId]);
  useEffect(() => {
    if (loadedOnMountRef.current) return;
    loadedOnMountRef.current = true;
    if (!urlSessionId) return;
    const sessionId = urlSessionId;
    (async () => {
      try {
        const data = await apiGet<{ messages: ChatMessage[] }>(`/sessions/${sessionId}/messages`);
        if (activeUrlSessionRef.current === sessionId) {
          useAppStore.getState().setMessages(data.messages);
        }
      } catch {
        // 加载失败保持空列表（与 reloadSession 行为一致）
      }
    })();
  }, [urlSessionId]);

  // ADR-013：唤醒轮自动刷新——会话末尾是后台任务 System 通知时轮询会话消息，
  // 直到出现新的非空 assistant 消息（主 agent 自动汇总结果，无需用户操作）
  useEffect(() => {
    if (!currentSessionId || streamStatus === 'streaming') return;
    const last = messages[messages.length - 1];
    const hasTaskNotice = last?.role === 'system' && last.content.includes('后台任务');
    if (!hasTaskNotice) return;

    setWakePolling(true);
    const timer = setInterval(async () => {
      try {
        const data = await apiGet<{ messages: ChatMessage[] }>(
          `/sessions/${currentSessionId}/messages`,
        );
        const serverMessages = data.messages;
        const lastServer = serverMessages[serverMessages.length - 1];
        // 唤醒轮结果（非空 assistant）出现 → 刷新并停止轮询
        if (lastServer && lastServer.role === 'assistant' && lastServer.content !== '') {
          useAppStore.getState().setMessages(serverMessages);
          setWakePolling(false);
        }
      } catch {
        // 轮询失败（会话删除等）：停止，避免无限重试
        setWakePolling(false);
      }
    }, 3000);
    return () => {
      clearInterval(timer);
      setWakePolling(false);
    };
  }, [currentSessionId, messages, streamStatus]);

  // 应用层授权：轮询审批状态，当前会话有挂起操作时显示审批卡片。
  // wait_for_approval 模式下危险操作由应用审批（与会话/LLM 无关），
  // 批准/拒绝后挂起的工具自动继续。
  useEffect(() => {
    if (!currentSessionId) {
      setPendingApproval(null);
      return;
    }
    let cancelled = false;
    const poll = async () => {
      try {
        const status = await fetchApprovalStatus();
        if (cancelled) return;
        const mine =
          status.pending_approvals.find((p) => p.session_id === currentSessionId) ?? null;
        setPendingApproval((prev) => {
          if (prev?.request_id !== mine?.request_id) return mine;
          return prev;
        });
      } catch {
        /* 轮询失败静默（下次重试） */
      }
    };
    void poll();
    const timer = setInterval(poll, 4000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [currentSessionId]);

  const handleApproval = async (decision: ApprovalDecision) => {
    if (!pendingApproval || approvalBusy) return;
    setApprovalBusy(true);
    try {
      await respondApproval(pendingApproval.request_id, decision);
      setPendingApproval(null);
    } catch (err: unknown) {
      useAppStore
        .getState()
        .showToast(`审批响应失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
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

  // ─── Custom streaming via fetch + ReadableStream ─────────────────

  const { startStream, stopStream } = useChatStream({
    streamUrl: `${getApiBase()}/chat/stream`,
    onChunk: (event) => {
      // 会话守卫：残留 chunk（会话切换/界面切换后到达，abort 有竞态窗口）
      // 直接丢弃——否则思考/工具结果会追加到当前（错误）会话的消息列表
      const activeSession = sessionIdRef.current;
      if (event.session_id && activeSession && event.session_id !== activeSession) {
        return;
      }
      // Update session ID from server response
      if (event.session_id) {
        useAppStore.getState().setCurrentSession(event.session_id);
      }
      // Server-side error (validation / processing failure): surface to user
      if (event.chunk_type === 'error') {
        useAppStore.getState().removeEmptyAssistantMessage();
        useAppStore.getState().setStreamStatus('idle');
        useAppStore.getState().showToast(event.delta || '对话处理失败', 'error');
        return;
      }
      // Clarification: 不追加 delta，改为展示追问气泡等待用户回答
      if (event.chunk_type === 'clarification') {
        useAppStore.getState().removeEmptyAssistantMessage();
        useAppStore.getState().setPendingClarification(event.delta);
        return;
      }
      // Append content delta to the last assistant message
      if (event.delta) {
        useAppStore.getState().updateLastMessage(event.delta);
      }
      // 思考增量（Thought chunk）单独累积到 thinking 字段（与正文分开渲染）。
      // 轮次边界：新一轮思考到达且上一轮已产出内容（正文/工具调用）时，
      // 新开一条 assistant 消息——与历史加载“一轮一条消息”的渲染一致
      // （流式不再整个会话一个大框）。
      if (event.thinking) {
        const st = useAppStore.getState();
        const last = st.messages[st.messages.length - 1];
        const hasPrevTurn =
          last?.role === 'assistant' &&
          (last.content !== '' || (last.tool_calls && last.tool_calls.length > 0));
        if (hasPrevTurn) st.startNewAssistantTurn();
        useAppStore.getState().appendThinking(event.thinking);
      }
      // Attach skill calls to the current assistant message
      if (event.skill_calls && event.skill_calls.length > 0) {
        useAppStore.getState().appendSkillCalls(event.skill_calls);
      }
      // A2：结构化工具调用事件 → tool card 渲染（含展示意图）
      if (event.tool_call) {
        useAppStore.getState().appendToolCalls([event.tool_call]);
      }
      // 完成 chunk 携带 token 用量 → 附加到当前 assistant 消息（前端按会话取数，
      // 切换会话显示各自占用）；同时记录本次流式的真实窗口
      if (event.usage) {
        liveWindowRef.current = event.usage.context_window;
        const { prompt_tokens, completion_tokens, total_tokens, cache_read, cache_write } =
          event.usage;
        useAppStore.getState().attachLastMessageUsage({
          prompt_tokens,
          completion_tokens,
          total_tokens,
          cache_read,
          cache_write,
        });
      }
      // 输出达到 token 上限（finish_reason === 'length'）：标记消息为截断，
      // 在助手消息下方渲染提示；'stop'/'tool_calls' 等不处理
      if (event.finish_reason === 'length') {
        useAppStore.getState().markLastMessageTruncated();
      }
    },
    onError: (error) => {
      useAppStore.getState().setStreamStatus('idle');
      useAppStore.getState().showToast(`发送失败: ${error.message}`, 'error');
    },
    onComplete: () => {
      useAppStore.getState().setStreamStatus('idle');
    },
  });

  // 会话守卫镜像：currentSessionId 变化时同步 ref（SSE 回调读 ref 不闭包过期）
  useEffect(() => {
    sessionIdRef.current = currentSessionId;
  }, [currentSessionId]);

  // 会话切换或组件卸载：中止在途流——旧流的 chunk 不得写入新会话的消息列表
  // （此前无任何清理点，残留 chunk 会把思考/工具结果追加到错误的消息）
  useEffect(() => {
    return () => {
      stopStream();
    };
  }, [stopStream, currentSessionId]);

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

      // 乐观更新：回退到该消息之前（删除该消息及其后）
      deleteMessagesFrom(index);
      try {
        const resp = await deleteSessionMessage(sessionId, index);
        state.setMessages(resp.messages);
        setLastRollbackIndex(index);
      } catch (err: unknown) {
        state.showToast(`回退失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
        await reloadSession(sessionId);
      }
    },
    [streamStatus, deleteMessagesFrom, setLastRollbackIndex],
  );

  // 撤销回退：恢复被回退的消息与工作区文件
  const handleRedo = useCallback(async () => {
    if (streamStatus === 'streaming' || lastRollbackIndex === null) return;

    const state = useAppStore.getState();
    const sessionId = state.currentSessionId;
    if (!sessionId) return;

    const index = lastRollbackIndex;
    try {
      const resp = await redoSessionMessage(sessionId, index);
      state.setMessages(resp.messages);
      setLastRollbackIndex(null);
      state.showToast('已撤销回退', 'success');
    } catch (err: unknown) {
      state.showToast(`撤销回退失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
      await reloadSession(sessionId);
    }
  }, [streamStatus, lastRollbackIndex, setLastRollbackIndex]);

  // 从后端重新加载会话消息（失败回滚，恢复与持久化一致的状态）
  const reloadSession = useCallback(async (sessionId: string) => {
    try {
      const data = await apiGet<{ messages: ChatMessage[] }>(`/sessions/${sessionId}/messages`);
      useAppStore.getState().setMessages(data.messages);
    } catch {
      useAppStore.getState().setMessages([]);
    }
  }, []);

  // 提交对 Agent 追问的回答（流式）：确认后思考/工具/输出逐块渲染，
  // 避免整轮等待超过 HTTP 超时（此前非流式路径表现为"按钮转圈后报错"）。
  const handleClarify = useCallback(
    async (answer: string) => {
      if (streamStatus === 'streaming' || submittingClarify) return;
      const state = useAppStore.getState();
      const sessionId = state.currentSessionId;
      if (!sessionId) {
        state.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }
      addMessage({
        role: 'user',
        content: answer,
        timestamp: new Date().toISOString(),
      });
      // 助手占位消息：流式增量累积其上
      addMessage({
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
      });
      setSubmittingClarify(true);
      try {
        const response = await fetch(`${getApiBase()}/chat/clarify/stream`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ session_id: sessionId, answer }),
        });
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}`);
        }
        const reader = response.body?.getReader();
        if (!reader) throw new Error('Response body is not readable');

        const decoder = new TextDecoder();
        let buffer = '';
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split('\n');
          buffer = lines.pop() || '';
          for (const line of lines) {
            const trimmed = line.trim();
            if (!trimmed || !trimmed.startsWith('data: ')) continue;
            const data = trimmed.slice(6).trim();
            if (data === '[DONE]') continue;
            try {
              const event = JSON.parse(data) as ChatStreamEvent;
              if (event.thinking) useAppStore.getState().appendThinking(event.thinking);
              if (event.delta) useAppStore.getState().updateLastMessage(event.delta);
              if (event.tool_call) useAppStore.getState().appendToolCalls([event.tool_call]);
              // 工具结果事件：耗时/成败 + 结果内容（后端已随事件透传）挂到卡片
              if (event.tool_result) {
                useAppStore.getState().applyToolResult(event.tool_result);
              }
              if (event.skill_calls && event.skill_calls.length > 0) {
                useAppStore.getState().appendSkillCalls(event.skill_calls);
              }
              if (event.finish_reason === 'length') {
                useAppStore.getState().markLastMessageTruncated();
              }
              if (event.chunk_type === 'error') {
                useAppStore.getState().removeEmptyAssistantMessage();
                useAppStore.getState().showToast(event.delta || '追问回答失败', 'error');
              }
            } catch {
              /* skip malformed SSE data */
            }
          }
        }
        // 流正常结束：追问气泡消失
        useAppStore.getState().setPendingClarification(null);
      } catch (err) {
        useAppStore.getState().removeEmptyAssistantMessage();
        useAppStore
          .getState()
          .showToast(`追问回答失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
        await reloadSession(sessionId);
      } finally {
        setSubmittingClarify(false);
      }
    },
    [streamStatus, submittingClarify, addMessage, reloadSession],
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
      state.showToast(`压缩失败: ${err instanceof Error ? err.message : '未知错误'}`, 'error');
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
                思考已产出内容（message.thinking 非空）时由气泡内指示接管，
                避免转圈与气泡内“思考中 · N 字”重复。 */}
            {streamStatus === 'streaming' &&
              messages.length > 0 &&
              messages[messages.length - 1].role === 'assistant' &&
              messages[messages.length - 1].content === '' &&
              !messages[messages.length - 1].thinking && (
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
            {wakePolling && streamStatus === 'idle' && (
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

        {/* Clarification bubble: Agent 追问需要用户回答 */}
        {pendingClarification && streamStatus !== 'streaming' && (
          <div className="flex justify-end">
            <ClarificationBubble
              question={pendingClarification}
              submitting={submittingClarify}
              onSubmit={handleClarify}
            />
          </div>
        )}

        {/* Redo banner: 回退后可撤销 */}
        {lastRollbackIndex !== null && streamStatus !== 'streaming' && (
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

      {/* 应用层授权卡片：危险操作等待人工批准（与 LLM 澄清无关） */}
      {pendingApproval && (
        <div
          role="alert"
          aria-label="操作等待授权"
          className="mx-4 mb-2 p-3 rounded-lg border border-amber-500/40 bg-amber-50/60 dark:bg-amber-950/20 text-sm"
        >
          <div className="flex items-center gap-2 text-amber-700 dark:text-amber-400">
            <ShieldAlert size={16} className="shrink-0" />
            <span className="font-medium">操作等待授权</span>
            <span className="text-xs opacity-70 ml-auto">风险：{pendingApproval.risk_level}</span>
          </div>
          <p className="mt-1.5 text-xs font-mono text-[var(--color-text-primary)] break-all">
            {pendingApproval.action_description}
          </p>
          <div className="mt-2 flex items-center gap-2">
            <button
              type="button"
              onClick={() => handleApproval('approve')}
              disabled={approvalBusy}
              className="flex items-center gap-1 px-3 py-1.5 text-xs font-medium rounded-md bg-green-600 text-white hover:bg-green-700 disabled:opacity-50"
            >
              <Check size={12} />
              批准
            </button>
            <button
              type="button"
              onClick={() => handleApproval('deny')}
              disabled={approvalBusy}
              className="flex items-center gap-1 px-3 py-1.5 text-xs font-medium rounded-md bg-red-600 text-white hover:bg-red-700 disabled:opacity-50"
            >
              <X size={12} />
              拒绝
            </button>
            <span className="text-xs text-[var(--color-text-tertiary)]">
              批准后挂起的操作将自动继续执行
            </span>
          </div>
        </div>
      )}

      {/* Input area（模型/思考强度/上下文圆环 + 发送：DSH 布局） */}
      <ChatInput
        onSend={handleSend}
        onStop={handleStop}
        isStreaming={streamStatus === 'streaming'}
        usage={lastUsage}
        sessionUsage={sessionUsage}
        onCompress={() => void handleCompress()}
      />
    </div>
  );
}
