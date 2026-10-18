import { useRef, useEffect, useCallback, useState, useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso';
import { useAppStore } from '@/lib/store';
import {
  ApiError,
  answerChat,
  cancelChatStream,
  compressSession,
  deleteSessionMessage,
  fetchApprovalStatus,
  getApiBase,
  getSessionModel,
  redoSessionMessage,
  respondApproval,
  setSessionModel,
} from '@/lib/api-client';
import type {
  ApprovalDecision,
  ApprovalStatusSnapshot,
  ChatMessage,
  ToolCallEvent,
} from '@/lib/types';
import { messageText } from '@/lib/types';
import { useChatStream } from '@/hooks/useChatStream';
import { cancelWhenSessionIdReady } from '@/lib/cancel-pending';
import { usePolling } from '@/hooks/use-polling';
import { useSessionHistory } from '@/hooks/use-session-history';
import { useUnifiedEvents, subscribeSession } from '@/hooks/use-unified-events';
import { toErrorMessage } from '@/lib/errors';
import { lastMessageUsage } from '@/lib/token-usage';
import { refreshSessionTokenUsage } from '@/lib/session-usage';
import { MessageSquare, Loader2, RotateCcw, Undo2, X } from 'lucide-react';
import ChatInput from './ChatInput';
import ClarificationBubble from './ClarificationBubble';
import MessageBubble from './MessageBubble';
import { streamingIndicatorOwner } from './streaming-indicator';
import ApprovalBanner from './ApprovalBanner';
import AgentTasksPanel from './AgentTasksPanel';
import SessionTodoPanel from './SessionTodoPanel';
import SessionTabBar from './SessionTabBar';
import { PENDING_SESSION_KEY } from '@/lib/store';

/** 虚拟列表首项索引基数（远大于任何会话的消息条数；上滚前插时递减）。 */
const FIRST_INDEX_BASE = 1_000_000;

/** 控制面操作错误文案（ADR-041）：忙语义按结构化 reason 分流，其余走通用。 */
function controlOpErrorMessage(err: unknown): string {
  if (err instanceof ApiError && err.reason) {
    switch (err.reason) {
      case 'stream_in_progress':
        return '该会话正在生成回复，已请求停止，请稍后再试';
      case 'destructive_op_in_progress':
        return '该会话有正在处理的会话操作（回退/重做/压缩），请稍候';
      case 'same_operation_in_flight':
        return '同一次操作仍在处理中，请稍候';
      default:
        break;
    }
  }
  return toErrorMessage(err, '未知错误');
}

export default function ChatPanel() {
  const { sessionId: urlSessionId } = useParams<{ sessionId: string }>();

  // Store state - individual selectors for minimal re-renders
  const messages = useAppStore((s) => s.messages);
  const streamStatusMap = useAppStore((s) => s.streamStatus);
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  /** 当前会话流式状态（按会话归属：切到原会话的流继续，互不阻塞） */
  const streamStatus = streamStatusMap[currentSessionId ?? PENDING_SESSION_KEY] ?? 'idle';
  /** 后端轮状态（ADR-035 §9 / U10）：唤醒轮/后台轮运行中——输入禁用 + 停止按钮
   * （B 方案：不出现"打了字发不出去"的困惑态）。streamStatus 只反映用户发起的
   * 流（覆盖不了唤醒轮），轮状态以 turn_state 事件为权威。 */
  const turnStateMap = useAppStore((s) => s.turnState);
  const turnRunning =
    (turnStateMap[currentSessionId ?? PENDING_SESSION_KEY]?.state ?? 'idle') === 'running';
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const addMessage = useAppStore((s) => s.addMessage);
  const setStreamStatus = useAppStore((s) => s.setStreamStatus);
  const lastRollbackMessageId = useAppStore((s) =>
    s.currentSessionId ? (s.rollbackMessageBySession[s.currentSessionId] ?? null) : null,
  );
  const setRollbackMessage = useAppStore((s) => s.setRollbackMessage);
  const eventsConnected = useAppStore((s) => s.eventsConnected);
  /** 当前会话的待回答追问（per-session）：切到哪个会话，哪个会话的问题接管输入框。 */
  const currentClarification = useAppStore(
    (s) => s.pendingClarifications[s.currentSessionId ?? ''],
  );

  /** 虚拟列表句柄（react-virtuoso）：程序化滚动到最新（替代 scrollTop 直写）。 */
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  /** 虚拟列表首项索引（prepend 锚定）：初始基数，上滚前插 N 条后 -=N——
      Virtuoso 以 firstItemIndex+index 对账项键，前插保持视口不跳。 */
  const [firstItemIndex, setFirstItemIndex] = useState(FIRST_INDEX_BASE);
  const [compressing, setCompressing] = useState(false);
  const [clarifySubmitting, setClarifySubmitting] = useState(false);
  /** 是否跟随底部（聊天经典模式）：用户在底部时自动跟随新输出；
      向上滚动超过阈值立即脱离跟随（不再被拉回），滚回底部恢复。
      由 Virtuoso `atBottomStateChange` 维护（滚动管理内聚到虚拟列表）。 */
  const stickToBottomRef = useRef(true);
  /** 上滚加载中（防重复触发；ADR-035 §8）。 */
  const loadingOlderRef = useRef(false);
  /** 内容跟随签名：消息数/流状态/会话切换变化时触发一次滚到最新
      （scrollToIndex LAST）；同消息内容增高的粘底由 Virtuoso 内部处理。 */
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
  /** 会话级轮错误（LLM 请求失败等）：持久横幅（内存态，不落库，重启即清）。 */
  const turnError = useAppStore(
    (s) => s.turnErrorBySession[currentSessionId ?? PENDING_SESSION_KEY] ?? null,
  );
  /** 会话级「正在停止」（点击停止 → 后端收尾完成之间；与 turnError 同款内存态）。 */
  const stopping = useAppStore(
    (s) => s.stoppingBySession[currentSessionId ?? PENDING_SESSION_KEY] ?? false,
  );

  /** 当前会话的系统前缀基线（后端列表下发 `first_request_tokens`；压缩后占用估算的取数点） */
  const sessionBaselineTokens = useAppStore(
    (s) => s.sessions.find((x) => x.id === s.currentSessionId)?.first_request_tokens,
  );

  /** 当前会话自己的上下文占用（lib/token-usage 纯函数；切换会话随 messages 变化） */
  const lastUsage = useMemo(
    () =>
      lastMessageUsage(
        messages,
        chatModels.find(
          (m) => m.provider === selectedModel?.provider && m.name === selectedModel?.model,
        )?.context_length ?? liveWindowRef.current,
        sessionBaselineTokens,
      ),
    [messages, chatModels, selectedModel, sessionBaselineTokens],
  );

  /** 当前会话 token 消耗汇总（后端 usage_logs 全量聚合；打开/新消息后刷新） */
  const sessionUsage = useAppStore((s) =>
    s.currentSessionId ? (s.sessionTokenUsage[s.currentSessionId] ?? null) : null,
  );

  // 会话 token 汇总刷新（底部统计条）：打开/切换立即拉取（250ms 防抖，
  // 与「消息数变化」共用同一 effect——新轮落库/压缩点插入也触发）。
  // 数据源为后端全量聚合，与分页加载窗口无关。
  useEffect(() => {
    if (!currentSessionId) return;
    const timer = setTimeout(() => void refreshSessionTokenUsage(currentSessionId), 250);
    return () => clearTimeout(timer);
  }, [currentSessionId, messages.length]);

  // Sync URL sessionId to store on mount / navigation
  // （per-session 追问不随切换清除：各会话条目独立留存，切回即恢复接管）
  useEffect(() => {
    if (urlSessionId && urlSessionId !== currentSessionId) {
      setCurrentSession(urlSessionId);
    }
  }, [urlSessionId, currentSessionId, setCurrentSession]);

  // 会话级模型选择：切会话同步视图（有缓存用缓存 / 无记录回落全局默认）；
  // 缓存缺失时拉取（按会话寻址写入——迟到响应只更新目标会话，不污染已切走的）
  useEffect(() => {
    const key = currentSessionId ?? PENDING_SESSION_KEY;
    const st = useAppStore.getState();
    st.syncModelView(key);
    if (currentSessionId && !st.sessionModelSelections[currentSessionId]) {
      const sid = currentSessionId;
      void getSessionModel(sid)
        .then((selection) => {
          if (!selection) return;
          const cur = useAppStore.getState();
          cur.setSessionSelection(sid, selection);
          // 仅当仍是当前会话时同步视图（迟到响应不污染）
          if (cur.currentSessionId === sid) cur.syncModelView(sid);
        })
        .catch(() => {
          /* 读取失败：保持默认视图（不阻塞） */
        });
    }
  }, [currentSessionId]);

  // 历史加载（挂载恢复 + reloadSession）收敛在 use-session-history
  const { reloadSession, loadOlder } = useSessionHistory(urlSessionId);

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

  // ── 虚拟列表滚动管理（react-virtuoso：滚动/跟随/前插锚定内聚） ─────

  /** 上滚到顶：加载更早一页（ADR-035 §8）。prepend 后同步递减首项索引——
      Virtuoso 以 firstItemIndex+index 对账项键，前插不改变既有项键，
      视口锚定由 Virtuoso 保持（替代浏览器原生 overflow-anchor）。 */
  const tryLoadOlder = useCallback(() => {
    if (loadingOlderRef.current || !urlSessionId) return;
    const meta = useAppStore.getState().sessionMessageMeta[urlSessionId];
    if (!meta?.hasMore) return;
    loadingOlderRef.current = true;
    void loadOlder(urlSessionId)
      .then((loaded) => {
        if (loaded > 0) setFirstItemIndex((v) => v - loaded);
      })
      .finally(() => {
        loadingOlderRef.current = false;
      });
  }, [loadOlder, urlSessionId]);

  /** scroller 元素（"真到顶"判定用）：startReached 的 overscan 会在离顶
      还有一大截（数千像素）时就触发——表现为"稍微滚动就加载"；改由原生
      scroll 事件在 scrollTop≈0（滚到顶）时才取下一页。 */
  const [scrollerEl, setScrollerEl] = useState<HTMLElement | null>(null);
  const handleScrollerRef = useCallback((el: HTMLElement | Window | null) => {
    setScrollerEl(el instanceof HTMLElement ? el : null);
  }, []);

  // 滚到顶才加载：单页加载且锚定完成后 scrollTop 变为插入内容高度（数千
  // 像素），自然远离顶部——不会连发；用户继续上滚到底再取下一页。
  useEffect(() => {
    if (!scrollerEl) return;
    const onScroll = () => {
      if (scrollerEl.scrollTop <= 8) tryLoadOlder();
    };
    scrollerEl.addEventListener('scroll', onScroll, { passive: true });
    return () => scrollerEl.removeEventListener('scroll', onScroll);
  }, [scrollerEl, tryLoadOlder]);

  /**
   * 上滚**自愈**（ADR-035 §8 修订）：消息不足以撑出滚动条时，scroller 的
   * scrollTop 恒为 0、原生 scroll 事件永不触发——「要上滚才加载、内容太少
   * 又滚不动」形成死锁（用户报告：定时任务会话只显示 1 条、历史看不到且
   * 不能上滚）。
   *
   * 内容未撑满视口且仍有更早历史 → 主动补一页；补到撑满或链首即停。
   * 连续补齐上限 3 次（失败时不自陷重试风暴），按会话重置计数。
   */
  const autoFillRef = useRef<{ sid: string; count: number }>({ sid: '', count: 0 });
  useEffect(() => {
    if (!scrollerEl || !urlSessionId) return;
    if (autoFillRef.current.sid !== urlSessionId) {
      autoFillRef.current = { sid: urlSessionId, count: 0 };
    }
    const meta = useAppStore.getState().sessionMessageMeta[urlSessionId];
    const canScroll = scrollerEl.scrollHeight > scrollerEl.clientHeight + 8;
    if (!meta?.hasMore || canScroll) {
      autoFillRef.current.count = 0;
      return;
    }
    if (autoFillRef.current.count >= 3) return;
    autoFillRef.current.count += 1;
    tryLoadOlder();
  }, [scrollerEl, messages.length, urlSessionId, tryLoadOlder]);

  // 会话切换：重置跟随（切回预期看到最新内容）+ 重置首项索引基数
  // （Virtuoso 经 key 重建，initialTopMostItemIndex 定位到最新）。
  useEffect(() => {
    stickToBottomRef.current = true;
    setFirstItemIndex(FIRST_INDEX_BASE);
  }, [currentSessionId]);

  // 内容跟随签名：消息数/流状态/会话切换变化时，若仍跟随则滚到最新。
  // 签名去重避免重复滚动；同消息内容增高（流式）的粘底由 Virtuoso 的
  // followOutput 内部处理。
  const followSig = `${currentSessionId}:${messages.length}:${streamStatus}:${
    currentClarification ? 1 : 0
  }`;
  useEffect(() => {
    if (followSigRef.current === followSig) return;
    followSigRef.current = followSig;
    if (stickToBottomRef.current) {
      virtuosoRef.current?.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'auto' });
    }
  }, [followSig]);

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
      setRollbackMessage(activeKey, null);
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
        model: state.selectedModel?.model,
        // 会话级模型选择的路由提供商（跨 provider 同名模型消歧）
        provider: state.selectedModel?.provider,
        // 会话级思考强度（对话时选择；off 不附加思考参数，仅对支持思考的模型生效）
        thinking: state.thinkingEffort === 'off' ? undefined : state.thinkingEffort,
        // 新会话绑定工作区（工作区 = 会话的父级分组；服务端固化到会话头部）
        working_directory: isNewSession ? (state.newSessionWorkspace ?? undefined) : undefined,
      };
      // 清除上一次的断线标志
      setStreamError(false);
      // 清除轮错误横幅（新消息发出即视为已恢复）
      useAppStore.getState().setTurnError(state.currentSessionId ?? PENDING_SESSION_KEY, null);
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
          // 会话级模型选择：PENDING 槽（用户在新会话里显式选过）迁移并固化——
          // 未选过则保持"无记录"语义（跟随全局默认）
          const st = useAppStore.getState();
          const pendingSelection = st.sessionModelSelections[PENDING_SESSION_KEY];
          if (pendingSelection) {
            st.setSessionSelection(confirmedSessionId, pendingSelection);
            st.setSessionSelection(PENDING_SESSION_KEY, null);
            st.syncModelView(confirmedSessionId);
            void setSessionModel(confirmedSessionId, pendingSelection).catch(() => {
              /* 固化失败：本会话保持"无记录"（跟随默认），不阻塞 */
            });
          }
        }
      } else {
        // 启动失败：复位流状态 + 清理占位（事件不会到达）
        useAppStore.getState().setStreamStatus('idle');
        useAppStore.getState().removeEmptyAssistantMessage();
      }
      // 新会话已创建并固化工作区绑定：待绑定状态由会话列表在刷新就绪后清理
      // （U9：此处不再立即清——列表刷新是异步的，立即清会让新建工作目录
      // 的分组在刷新返回前失去依据闪断；失败路径也不应清，否则重试丢目录）。
      // 见 SessionList 的 currentSessionId 变更 effect。
    },
    [addMessage, startStream, setRollbackMessage],
  );

  // ── 控制面操作（ADR-041 波次 4）──
  // 回退/重做进行中：输入禁用 + 拒绝重复发起（服务端已有 opId 幂等 + 租约互斥，
  // 前端这层是 UX 与迟到响应守卫）。busy 用 ref 守卫（不破坏回调身份稳定性）、
  // state 供渲染。
  const [controlBusy, setControlBusy] = useState<'rollback' | 'redo' | null>(null);
  const controlBusyRef = useRef(false);
  /** 每会话单调递增的操作序号：迟到响应若已被更新的操作取代 → 丢弃。 */
  const controlOpSeqRef = useRef<Record<string, number>>({});
  const beginControlOp = useCallback((sessionId: string) => {
    const next = (controlOpSeqRef.current[sessionId] ?? 0) + 1;
    controlOpSeqRef.current[sessionId] = next;
    return next;
  }, []);
  const isCurrentControlOp = useCallback(
    (sessionId: string, seq: number) => controlOpSeqRef.current[sessionId] === seq,
    [],
  );

  const handleRollback = useCallback(
    async (index: number) => {
      // 读 store 最新值而非渲染闭包：回调身份稳定（不依赖 messages/streamStatus），
      // memo(MessageBubble) 的 onRollback 浅比较不失效——流式事件逐条到达时
      // 只有变更气泡重渲染，避免大会话全量重渲染卡死（O(n²) markdown/高亮）。
      const st = useAppStore.getState();
      const activeKey = st.currentSessionId ?? PENDING_SESSION_KEY;
      if ((st.streamStatus[activeKey] ?? 'idle') === 'streaming') return;
      if (controlBusyRef.current) return; // 控制面操作进行中：拒绝重复发起
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
      const seq = beginControlOp(sessionId);
      const opId = `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
      controlBusyRef.current = true;
      setControlBusy('rollback');
      st.deleteMessagesFrom(sessionId, index);
      try {
        const resp = await deleteSessionMessage(sessionId, target.id, opId);
        // 迟到响应守卫（ADR-041 §4）：已被更新操作取代 → 丢弃（不覆盖本地）
        if (!isCurrentControlOp(sessionId, seq)) return;
        // 迟到响应写回发起会话（用户可能在请求期间切换会话——不得污染）
        st.setSessionMessages(sessionId, resp.messages);
        st.setRollbackMessage(sessionId, target.id);
        // 回退点用户消息回填该会话草稿（编辑重发语义）：文本 + 图片（data
        // URL）随回退带出，输入框直接可编辑重发；两者皆空时 setInputDraft
        // 自动删键（不产生空草稿）
        st.setInputDraft(sessionId, {
          text: messageText(target),
          images: target.images ?? [],
        });
      } catch (err: unknown) {
        st.showToast(`回退失败: ${controlOpErrorMessage(err)}`, 'error');
        await reloadSession(sessionId);
      } finally {
        controlBusyRef.current = false;
        setControlBusy(null);
      }
    },
    [reloadSession, beginControlOp, isCurrentControlOp],
  );

  // 撤销回滚：恢复被删除的消息与工作区文件
  const handleRedo = useCallback(async () => {
    if (streamStatus === 'streaming' || lastRollbackMessageId === null) return;
    if (controlBusyRef.current) return;

    const state = useAppStore.getState();
    const sessionId = state.currentSessionId;
    if (!sessionId) return;

    const seq = beginControlOp(sessionId);
    const opId = `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
    controlBusyRef.current = true;
    setControlBusy('redo');
    try {
      const resp = await redoSessionMessage(sessionId, lastRollbackMessageId, opId);
      if (!isCurrentControlOp(sessionId, seq)) return;
      // 迟到响应写回发起会话（撤销回退同理按会话 id 寻址）
      state.setSessionMessages(sessionId, resp.messages);
      setRollbackMessage(sessionId, null);
      state.showToast('已撤销回退', 'success');
    } catch (err: unknown) {
      state.showToast(`撤销回退失败: ${controlOpErrorMessage(err)}`, 'error');
      await reloadSession(sessionId);
    } finally {
      controlBusyRef.current = false;
      setControlBusy(null);
    }
  }, [
    streamStatus,
    lastRollbackMessageId,
    setRollbackMessage,
    reloadSession,
    beginControlOp,
    isCurrentControlOp,
  ]);

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
      // per-session：气泡按当前会话取用，回答提交回该会话
      // （错投会让原会话的 ask_user 永久挂起）。
      const sessionId = state.currentSessionId;
      if (!sessionId || !state.pendingClarifications[sessionId]) {
        if (!sessionId) state.showToast('请先发送一条消息以创建会话', 'error');
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
        // 回答已受理：清除会话列表问号标识（turn 仍 running，其余状态保留）
        const answeredTurn = useAppStore.getState().turnState[sessionId];
        if (answeredTurn) {
          useAppStore.getState().setTurnState(sessionId, {
            ...answeredTurn,
            pendingAsk: false,
          });
        }
        state.clearPendingClarification(sessionId);
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
    // 「正在停止」态键：sessionId 未就绪时落在新会话占位键（补发取消后仍清它）
    const stopKey = sid ?? PENDING_SESSION_KEY;
    // 置位「正在停止」：后端收尾（turn_state idle 事件）到达时清除；端点
    // 返回 no_active_stream（无活动轮）/请求失败时立即清除。
    useAppStore.getState().setStopping(stopKey, true);
    const sendCancel = (id: string) => {
      const clearStopping = () => {
        const store = useAppStore.getState();
        store.setStopping(id, false);
        store.setStopping(stopKey, false);
      };
      void cancelChatStream(id)
        .then((r) => {
          if (r.status === 'no_active_stream') clearStopping();
        })
        .catch(clearStopping);
    };
    if (sid) {
      sendCancel(sid);
    } else {
      // 竞态修复：新会话的首个请求尚未返回、sessionId 还没确定时点「停止」，
      // 取消端点无 id 可用 → 后端会按「跑完再取」继续跑完，前端却已显示
      // "已中止"（下次加载突然冒出完整回答）。此处等待 id 就绪后补发取消。
      cancelWhenSessionIdReady(() => useAppStore.getState().currentSessionId, sendCancel);
    }
    stopStream();
    setStreamStatus('idle');
    // 给当前流式消息打 interrupted 标记（与服务端 finish=interrupted 语义一致）
    useAppStore.getState().markLastMessageInterrupted(sid ?? undefined);
  }, [stopStream, setStreamStatus]);
  // 「正在停止」兜底：事件丢失 / 端点异常时最长 20s 后自行清除，防横幅悬挂。
  useEffect(() => {
    if (!stopping) return;
    const key = currentSessionId ?? PENDING_SESSION_KEY;
    const timer = window.setTimeout(() => {
      useAppStore.getState().setStopping(key, false);
    }, 20_000);
    return () => window.clearTimeout(timer);
  }, [stopping, currentSessionId]);

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
        {/* Header：「会话」标题 + 打开的会话 tab 条（浏览器式：打开顺序 / 可关闭）*/}
        <div className="flex items-center gap-3 px-4 py-2 border-b border-[var(--color-border)] shrink-0">
          <h1 className="text-lg font-semibold text-[var(--color-text-primary)] shrink-0">会话</h1>
          <SessionTabBar />
          <div className="flex items-center gap-2 shrink-0" />
        </div>

        {/* Messages area（虚拟列表：仅渲染视口附近消息——渲染量与消息总数解耦）
            水平留白由 item 内容层提供（下方 px-4）——不放 scroller：Virtuoso 的
            item 层 width:100% 相对 scroller 的 padding box 计算、却自内容盒
            左缘起布局，scroller 带水平 padding 时右侧恒定溢出 padding 宽度
            （16px）→ 消息区底部出现横向滚动条（实测 delta=16px 恒定）。 */}
        {messages.length === 0 && streamStatus === 'idle' && (
          <div className="flex-1 flex flex-col items-center justify-center text-[var(--color-text-tertiary)] gap-3">
            <MessageSquare className="w-12 h-12 opacity-30" />
            <p className="text-sm">开始一段新的对话</p>
            <p className="text-xs opacity-60">输入消息开始与 AI 助手交流</p>
          </div>
        )}

        {messages.length > 0 && (
          <Virtuoso
            key={currentSessionId ?? PENDING_SESSION_KEY}
            ref={virtuosoRef}
            data={messages}
            firstItemIndex={firstItemIndex}
            initialTopMostItemIndex={{ index: 'LAST', align: 'end' }}
            // 预渲染缓冲（条目式）：视口外前后各多渲染若干条，让「挂载 →
            // 高度测量」发生在进入视口之前——消息高度差异极大（短消息几十
            // 像素、长报告数千像素），无缓冲时每滚过一条都会「渲染即测量
            // 即位置修正」，上滚过程持续可见跳动/回弹。条目式（而非像素式
            // increaseViewportBy）对超高消息同样有效（库内说明：dynamic or
            // very tall content 推荐 minOverscanItemCount）。
            minOverscanItemCount={{ top: 5, bottom: 2 }}
            // 跳过「首条探针」并统一初始估算高度：默认行为用**第一个渲染
            // 项**做探针，遇 outlier（本场景实测 65~2337px）时全列表估算
            // 失真——滚动/跳变到未测区域时 Virtuoso 位置修正回弹可达
            // 1200px+（实测把视口内容顶回 4 条消息）。375 ≈ 实测均值。
            defaultItemHeight={375}
            followOutput={(isAtBottom) => (isAtBottom ? 'auto' : false)}
            atBottomStateChange={(atBottom) => {
              stickToBottomRef.current = atBottom;
            }}
            atBottomThreshold={24}
            scrollerRef={handleScrollerRef}
            className="flex-1 min-h-0 pt-4"
            itemContent={(i, msg) => {
              // Virtuoso 的 itemContent 索引是「全局索引」= 数据数组下标 + firstItemIndex
              // （1e6 基数）——回退定位（messages[index]）与流式判定（streamingIndex 为
              // 数组下标）都必须用真实下标，故换算回数组下标再传入；直用全局索引会
              // 使回退取到 undefined（报「缺少 ID」）、isStreaming 恒 false（气泡内
              // 思考指示消失）。测试 mock 已同步此库语义（react-virtuoso.tsx）。
              const index = i - firstItemIndex;
              return (
                <div className="max-w-4xl mx-auto pb-4 px-4">
                  <MessageBubble
                    message={msg}
                    index={index}
                    isStreaming={index === streamingIndex && msg.role === 'assistant'}
                    onRollback={handleRollback}
                  />
                </div>
              );
            }}
          />
        )}

        {/* 状态提示带（固定于消息区与输入区之间，不随列表滚动）：流式/压缩
            进度、断线、停止、控制面与轮错误等即时状态。
            水平对齐与消息列同构（w-full + max-w-4xl mx-auto + px-4）——此前
            无宽度约束时左对齐项贴列表左缘（「思考中」跑到最左边）。w-full
            必要：flex item 带 auto margin（mx-auto）时不参与 stretch，无
            显式宽度收缩为 fit-content（实测 32px），auto margin 无剩余空间
            可分配、不会居中。 */}
        <div className="shrink-0 w-full max-w-4xl mx-auto px-4">
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

          {/* 压缩中：进行中提示（与压缩按钮/输入框禁用同一状态源，
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

          {/* 轮级失败提示（LLM 请求失败等）：纯前端内存态——不落库、
                  不污染 LLM 上下文、应用重启即清；发送新消息或手动关闭后消失 */}
          {/* 「正在停止」过渡提示（纯前端内存态）：点击停止 → 后端收尾完成
              （turn_state idle）之间——收尾窗口给出明确反馈（不再"按了没反应"）。 */}
          {stopping && (
            <div className="flex items-center gap-2 py-2" role="status">
              <span className="text-xs text-[var(--color-text-secondary)]">正在停止…</span>
            </div>
          )}

          {/* 控制面操作进行中（ADR-041）：回退/重做期间输入禁用 + 明确的进行中反馈
             （消除「点了没反应 / 重复点击」）。 */}
          {controlBusy !== null && (
            <div className="flex items-center gap-2 py-2" role="status">
              <Loader2 className="w-4 h-4 animate-spin text-[var(--color-text-secondary)]" />
              <span className="text-xs text-[var(--color-text-secondary)]">
                {controlBusy === 'rollback' ? '回退处理中…' : '撤销回退处理中…'}
              </span>
            </div>
          )}

          {turnError && streamStatus !== 'streaming' && (
            <div className="flex items-center gap-2 py-2" role="alert">
              <span className="text-xs text-[var(--color-error)]">上一轮中断：{turnError}</span>
              <button
                type="button"
                onClick={() =>
                  currentSessionId && useAppStore.getState().setTurnError(currentSessionId, null)
                }
                className="flex items-center gap-1 px-2.5 py-1 text-xs rounded border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
                aria-label="关闭中断提示"
              >
                <X size={12} />
                关闭
              </button>
            </div>
          )}

          {/* T1-8：事件连接断开提示（状态由订阅快照的轮状态权威值校正） */}
          {!eventsConnected && (
            <div className="flex justify-center pb-1">
              <div className="flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-full border border-amber-500/40 bg-amber-50/60 dark:bg-amber-950/20 text-amber-700 dark:text-amber-300">
                实时连接断开，重连中…（回复可能延迟显示）
              </div>
            </div>
          )}

          {/* Redo banner: 回退后可撤销。
             ADR-041：不再依赖 streamStatus——旧条件在流状态未复位时把横幅压住
             （「回退后横幅延迟出现」的症状来源之一）。 */}
          {lastRollbackMessageId !== null && (
            <div className="flex justify-center pb-1">
              <button
                onClick={handleRedo}
                disabled={controlBusy !== null}
                className="flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-full border border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                aria-label="撤销回退"
              >
                <Undo2 className="w-3.5 h-3.5" />
                {controlBusy === 'redo' ? '撤销中…' : '已回退 — 撤销回退'}
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
        {currentClarification ? (
          <ClarificationBubble
            questions={currentClarification.questions}
            submitting={clarifySubmitting}
            onSubmit={handleClarify}
          />
        ) : (
          <ChatInput
            onSend={handleSend}
            onStop={handleStop}
            isStreaming={streamStatus === 'streaming'}
            busy={controlBusy !== null}
            turnRunning={turnRunning}
            stopping={stopping}
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
