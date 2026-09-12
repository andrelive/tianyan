import { useRef, useEffect, useCallback, useState, useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import {
  answerChat,
  cancelChatStream,
  compressSession,
  deleteSessionMessage,
  fetchApprovalStatus,
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
import { useUnifiedEvents, subscribeSession } from '@/hooks/use-unified-events';
import { toErrorMessage } from '@/lib/errors';
import { lastMessageUsage, sumSessionUsage } from '@/lib/token-usage';
import { MessageSquare, Loader2, RotateCcw, Undo2 } from 'lucide-react';
import ChatInput from './ChatInput';
import ClarificationBubble from './ClarificationBubble';
import MessageBubble from './MessageBubble';
import { streamingIndicatorOwner } from './streaming-indicator';
import ApprovalBanner from './ApprovalBanner';
import AgentTasksPanel from './AgentTasksPanel';
import SessionTodoPanel from './SessionTodoPanel';
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
  const lastRollbackMessageId = useAppStore((s) => s.lastRollbackMessageId);
  const setLastRollbackMessageId = useAppStore((s) => s.setLastRollbackMessageId);
  const pendingClarification = useAppStore((s) => s.pendingClarification);
  const setPendingClarification = useAppStore((s) => s.setPendingClarification);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [compressing, setCompressing] = useState(false);
  const [clarifySubmitting, setClarifySubmitting] = useState(false);
  /** 是否跟随底部（聊天经典模式）：用户在底部时自动跟随新输出；
      向上滚动超过阈值立即脱离跟随（不再被拉回），滚回底部恢复。
      scroll 热路径用 ref 避免高频 setState（DSH atBottomRef 双轨模式）。 */
  const stickToBottomRef = useRef(true);
  /** 程序化滚动账本：每次写 scrollTop 同步记录，scroll 事件据此判定
      "用户滚动"（|scrollTop - observedTop| > 0.5）——程序化滚动/浏览器
      clamp 不改变跟随所有权（DSH observed-top ledger 模式）。 */
  const observedTopRef = useRef(0);
  /** 内容跟随签名：仅当内容真正变化（消息数/流状态/会话切换）时触发
      follow——滚动阈值翻转（setState → effect → scrollToBottom → scroll）
      的循环不再发生（DSH followSig 模式）。 */
  const followSigRef = useRef('');
  /** 当前会话待审批操作（应用层授权卡片；交互模式挂起时出现） */
  const [pendingApproval, setPendingApproval] = useState<
    ApprovalStatusSnapshot['pending_approvals'][number] | null
  >(null);
  const [approvalBusy, setApprovalBusy] = useState(false);
  const selectedModel = useAppStore((s) => s.selectedModel);
  const chatModels = useAppStore((s) => s.chatModels);
  /** 最近一次流式完成 chunk 携带的真实窗口（会话流式时的权威值；
      历史会话回退到 chatModels[selectedModel].context_length）。 */
  const liveWindowRef = useRef(0);
  /** 流式中途断线（网络错误）标志：显示「任务继续在后台运行」提示。 */
  const [streamError, setStreamError] = useState(false);

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

  // ADR-028：统一事件订阅——后台通知/唤醒轮结果/任务状态经常驻 SSE 推送，
  // ADR-031：消息不再广播——流式增量 + 完成事件到前端；后台任务完成
  // 通知只落库（LLM 上下文），唤醒轮流式化后输出经 chat_stream 推送
  // （前端打字机看到汇总，无需"通知气泡 + 唤醒指示"轮询逻辑）。
  useUnifiedEvents();

  // 应用层授权：轮询审批状态，当前会话有挂起操作时显示审批卡片。
  // 交互模式下危险操作由应用审批（与会话/LLM 无关），
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

  // ── 滚动跟随（DSH observed-top ledger + followSig 模式） ─────────
  const FOLLOW_THRESHOLD = 24;

  /** 滚动到底：写 scrollTop 后同步记录账本（程序化滚动不改变跟随所有权）。 */
  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    observedTopRef.current = el.scrollTop;
  }, []);

  /** 滚动监听：仅用户输入（wheel/touch/scrollbar/键盘）改变原始滚动几何
      时更新跟随所有权；程序化滚动落在账本上，保持当前所有权。 */
  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const floor = Math.max(0, el.scrollHeight - el.clientHeight);
    const movedByReader = Math.abs(el.scrollTop - Math.min(observedTopRef.current, floor)) > 0.5;
    const isAtBottom = movedByReader
      ? floor - el.scrollTop <= FOLLOW_THRESHOLD + 1
      : stickToBottomRef.current;
    if (!movedByReader && isAtBottom) {
      // 程序化滚动落在账本上且已在底部：保持跟随（不重复滚动）
      observedTopRef.current = el.scrollTop;
      return;
    }
    stickToBottomRef.current = isAtBottom;
    observedTopRef.current = el.scrollTop;
  }, []);

  // 会话切换时重置跟随（切回预期看到最新内容）
  useEffect(() => {
    stickToBottomRef.current = true;
  }, [currentSessionId]);

  // 内容跟随签名：消息数/流状态/会话切换变化时，若仍跟随则滚动到底。
  // 滚动阈值翻转（setState → effect → scrollToBottom → scroll）不再触发
  // follow——签名只在内容真正变化时更新（DSH followSig 模式）。
  const followSig = `${currentSessionId}:${messages.length}:${streamStatus}:${pendingClarification ? 1 : 0}`;
  useEffect(() => {
    if (followSigRef.current === followSig) return;
    followSigRef.current = followSig;
    if (stickToBottomRef.current) scrollToBottom();
  }, [followSig, scrollToBottom]);

  // ResizeObserver：异步内容（图片/markdown/代码高亮）加载导致列高度
  // 变化时，若仍跟随则滚动到底（替代 setTimeout 兜底，DSH 同款）。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (stickToBottomRef.current) scrollToBottom();
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [scrollToBottom]);

  // ─── Custom streaming via fetch + ReadableStream ─────────────────

  // ADR-028 第 3 步：POST /chat/stream 收敛为开关——启动请求立即返回
  // session_id；流式输出（增量/边界/工具/usage）全部经 GET /events 统一
  // 事件通道下发，由 useUnifiedEvents 路由到活跃归约器（lib/chat-stream）。
  // 本组件只负责启动请求 + 注册/注销归约器。
  const { startStream, stopStream } = useChatStream({
    streamUrl: `${getApiBase()}/chat/stream`,
    onError: () => {
      // 启动失败（HTTP 错误）：复位流状态 + 清理占位
      useAppStore.getState().setStreamStatus('idle');
      useAppStore.getState().removeEmptyAssistantMessage();
      setStreamError(true);
    },
  });

  // 切走会话/界面后流继续在后台跑（fetch 循环持有归约器闭包，事件按
  // session_id 写入各自会话缓存）——不在此中止流，切回时直接显示累积内容。

  // ─── Handlers ───────────────────────────────────────────────────

  const handleSend = useCallback(
    async (content: string, images: string[] = []) => {
      // 同步守卫：读 store 最新值而非渲染闭包，防同一帧双击/连按并发两条流
      const st = useAppStore.getState();
      const activeKey = st.currentSessionId ?? PENDING_SESSION_KEY;
      if ((st.streamStatus[activeKey] ?? 'idle') === 'streaming') return;
      const trimmed = content.trim();
      if (!trimmed && images.length === 0) return;

      // 发起新轮：回撤已被新工作取代，清空撤销回退横幅（否则残留到输出底部）
      setLastRollbackMessageId(null);
      // 用户发送新消息：恢复底部跟随（此前可能向上回读）
      stickToBottomRef.current = true;

      // ADR-031：乐观渲染——本地立即插入用户消息（带 user_message_id 定位
      // 键），服务端落库后经 UserMessageId 确认事件回显真实 id，前端比对
      // user_message_id 精确替换（不再依赖"广播插入 + id 去重"的模糊匹配，
      // 也消除了 b749eb0 时代"本地 randomUUID 与广播 id 不一致导致重复"的
      // 根因——确认事件是同一消息的精确收尾）。
      const userMessageId = crypto.randomUUID();
      addMessage({
        role: 'user',
        segments: [{ type: 'text', text: trimmed }],
        user_message_id: userMessageId,
        // id: null = 占位（等待 UserMessageId 确认事件替换为服务端真实 id）
        id: null,
        images: images.length > 0 ? images : undefined,
        timestamp: new Date().toISOString(),
      });

      // Add empty assistant placeholder for streaming（id: null = 占位，
      // 等待服务端广播/边界事件替换为真实 id）
      addMessage({
        role: 'assistant',
        segments: [],
        id: null,
        timestamp: new Date().toISOString(),
      });

      // Build the API payload: only the new user message. The server's context
      // truth is the VFS-backed session history — sending the full local array
      // was dead weight and coupled the payload to the empty assistant
      // placeholder being the last element.
      const state = useAppStore.getState();
      const isNewSession = !state.currentSessionId;
      const payload = {
        session_id: state.currentSessionId,
        message: {
          role: 'user' as const,
          content: trimmed,
          user_message_id: userMessageId,
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
      };
      // 清除上一次的断线标志
      setStreamError(false);
      state.setStreamStatus('streaming');
      // ADR-028 第 3 步：启动请求立即返回 session_id；流式事件经统一事件
      // 通道（GET /events）到达——纯函数 handleChatStreamEvent 常驻处理
      // （无归约器注册表：事件自带 session_id 直接路由到 store，唤醒轮/
      // 后台事件同样可达）。
      const confirmedSessionId = await startStream(payload);
      if (confirmedSessionId) {
        // ADR-029：确保会话已订阅（快照恢复；resident 幂等——
        // 已订阅跳过，未订阅首次打开 → 快照补充）
        void subscribeSession(confirmedSessionId);
        // 新会话：服务端已确认 session_id，立即迁移（不等首个事件——
        // 事件可能因通道 Lag 丢失，PENDING 滞留会断链）
        if (isNewSession) {
          useAppStore.getState().setCurrentSession(confirmedSessionId);
        }
      } else {
        // 启动失败：复位流状态 + 清理占位（事件不会到达）
        useAppStore.getState().setStreamStatus('idle');
        useAppStore.getState().removeEmptyAssistantMessage();
      }
      // 新会话已创建并固化工作区绑定：清除待绑定状态（每个新对话重新选择）
      if (isNewSession) {
        useAppStore.getState().setNewSessionWorkspace(null);
      }
    },
    [addMessage, startStream, setLastRollbackMessageId],
  );

  const handleRollback = useCallback(
    async (index: number) => {
      // 读 store 最新值而非渲染闭包：回调身份稳定（不依赖 messages/streamStatus），
      // memo(MessageBubble) 的 onRollback 浅比较不失效——流式事件逐条到达时
      // 只有变更气泡重渲染，避免大会话全量重渲染卡死（O(n²) markdown/高亮）。
      const st = useAppStore.getState();
      const activeKey = st.currentSessionId ?? PENDING_SESSION_KEY;
      if ((st.streamStatus[activeKey] ?? 'idle') === 'streaming') return;
      const sessionId = st.currentSessionId;
      if (!sessionId) {
        st.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }

      // 被回退消息的 ID：重做数据的定位键（前端索引与服务端列表错位，
      // 数字索引不可靠——按 ID 定位删除）
      const target = st.messages[index];
      if (!target?.id) {
        st.showToast('该消息缺少 ID，无法回退', 'error');
        return;
      }

      // 乐观更新：回退到该消息之前（删除该消息及其后）
      st.deleteMessagesFrom(index);
      try {
        const resp = await deleteSessionMessage(sessionId, target.id);
        st.setMessages(resp.messages);
        st.setLastRollbackMessageId(target.id);
      } catch (err: unknown) {
        st.showToast(`回退失败: ${toErrorMessage(err, '未知错误')}`, 'error');
        await reloadSession(sessionId);
      }
    },
    [reloadSession],
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

  // 提交对 Agent 追问的回答（同步工具语义，对齐 DSH）：回答提交到
  // /chat/answer 等待通道，ask_user 工具执行恢复，结果经**主对话流**返回
  // （无独立澄清流）——模型看到 调用→结果 对后继续决策，最终完成 chunk
  // 复位流状态。提交失败保留接管组件（用户可重试）。
  const handleClarify = useCallback(
    async (answer: string) => {
      // 注意：同步工具语义下 ask_user 工具执行挂起时主对话流仍处于 streaming
      // （工具结果经原流返回）——提交回答不能因 streaming 被拦截，只防重复提交
      if (clarifySubmitting) return;
      const state = useAppStore.getState();
      const sessionId = state.currentSessionId;
      if (!sessionId) {
        state.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }
      setClarifySubmitting(true);
      try {
        // 回答 JSON（{answers, extra}）提交到等待通道
        const payload = JSON.parse(answer) as unknown;
        await answerChat(sessionId, payload);
        // 回答已受理：乐观挂到 ask_user 工具卡片（服务端 tool_result 到达后
        // 幂等覆盖）；接管组件退出，普通输入框恢复；主流保持 streaming
        // （工具执行挂起中，完成 chunk 到达后复位）
        const askCall = findAskUserCall(useAppStore.getState().messages);
        if (askCall && askCall.id) {
          useAppStore.getState().applyToolResult({
            tool_call_id: askCall.id,
            content: answer,
            success: true,
            duration_ms: 0,
          });
        }
        state.setPendingClarification(null);
      } catch (err: unknown) {
        state.showToast(`追问回答提交失败: ${toErrorMessage(err, '未知错误')}`, 'error');
      } finally {
        setClarifySubmitting(false);
      }
    },
    [clarifySubmitting],
  );

  const handleStop = useCallback(() => {
    // 主动停止：显式调用取消端点（跑完再取语义下断线不取消，只有主动停止才取消）
    const sid = useAppStore.getState().currentSessionId;
    if (sid) {
      void cancelChatStream(sid).catch(() => {});
    }
    stopStream();
    setStreamStatus('idle');
    // 给当前流式消息打 interrupted 标记（与服务端 finish=interrupted 语义一致）
    useAppStore.getState().markLastMessageInterrupted(sid ?? undefined);
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
      // 摘要消息收敛到消息流（展示始终只追加，保留完整历史；服务端上下文
      // 组装从压缩点开始与此无关）。走 applyServerMessage 按 id 幂等 upsert：
      // 与实时推送（chat_stream 边界事件）双路径收敛——先到者写入，后到者
      // 更新同一消息，不产生重复。
      if (resp.message) {
        useAppStore.getState().applyServerMessage(sessionId, resp.message);
      }
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
    <div className="flex h-full bg-[var(--color-bg-primary)]">
      {/* 左列：对话主区 */}
      <div className="flex flex-col flex-1 min-w-0">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] shrink-0">
          <h1 className="text-lg font-semibold text-[var(--color-text-primary)]">会话</h1>
          <div className="flex items-center gap-2" />
        </div>

        {/* Messages area */}
        <div ref={scrollRef} onScroll={handleScroll} className="flex-1 overflow-y-auto px-4 py-4">
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

              {/* 压缩中：底部进行中提示（与压缩按钮/输入框禁用同一状态源，
                压缩完成/失败后消失）。 */}
              {compressing && (
                <div
                  className="flex items-center justify-center gap-2 text-[var(--color-text-tertiary)] py-2"
                  aria-live="polite"
                  aria-label="正在压缩会话"
                >
                  <Loader2 className="w-4 h-4 animate-spin" />
                  <span className="text-sm">压缩中...</span>
                </div>
              )}

              {/* 断线提示：跑完再取——任务继续在后台运行，刷新获取最终结果 */}
              {streamError && streamStatus === 'idle' && (
                <div className="flex items-center gap-2 py-2" role="alert" aria-live="polite">
                  <span className="text-xs text-amber-600 dark:text-amber-400">
                    连接断开，任务继续在后台运行，完成后可刷新查看
                  </span>
                  <button
                    type="button"
                    onClick={() => currentSessionId && reloadSession(currentSessionId)}
                    className="flex items-center gap-1 px-2.5 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
                  >
                    <RotateCcw size={12} />
                    刷新结果
                  </button>
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

        {/* 会话内临时面板（完事即隐）：活跃待办/目标（todo/goal 工具产物）。 */}
        <SessionTodoPanel sessionId={currentSessionId} />

        {/* 应用层授权卡片（提取自 ChatPanel 内联；与 ApprovalPanel 共用语义） */}
        <ApprovalBanner
          approval={pendingApproval}
          busy={approvalBusy}
          onRespond={(decision) => void handleApproval(decision)}
        />

        {/* Input area（模型/思考强度/上下文圆环 + 发送：DSH 布局）
          追问待回答时由 ClarificationBubble 接管（composer takeover，对齐 DSH）：
          输入框区域被问题表单替代，回答提交后恢复。
          注意：同步工具语义下 ask_user 工具执行挂起时主对话流仍处于 streaming
          （工具结果经原流返回）——气泡渲染只看 pending，不再要求 idle。 */}
        {pendingClarification ? (
          <ClarificationBubble
            questions={pendingClarification.questions}
            submitting={clarifySubmitting}
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
            compressing={compressing}
          />
        )}
      </div>

      {/* 右列：会话后台任务面板（ADR-026：活跃在上、完成沉底、可展开/取消/折叠） */}
      <AgentTasksPanel sessionId={currentSessionId} />
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
