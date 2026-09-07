/**
 * 会话流式客户端 —— 统一事件通道（GET /events）事件 → store 动作的唯一定义点。
 *
 * 职责边界（深模块：小接口承载整个流式协议知识）：
 * - `createChatStreamReducer`：协议事件归约器，把每个 chunk 翻译为 store 动作
 *   （含会话归属、轮次边界、截断/中断语义、usage 归位、工具结果挂卡）。
 * - 活跃流归约器注册表：POST /chat/stream 启动后按 session_id 注册，
 *   `routeChatStreamEvent` 把统一事件通道的 chat_stream 事件路由到对应归约器。
 *
 * 主对话流（/chat/stream）与追问流共用同一归约器，组件只负责「启动请求」
 * 与「渲染」，不再内联协议知识。
 */

import { useAppStore } from '@/lib/store';
import type { ChatStreamEvent } from '@/lib/types';

/* ─────── ask_user 问题解析（同步工具语义） ─────── */

/**
 * 从 ask_user 工具调用参数（arguments JSON）解析问题列表。
 *
 * 参数形态（对齐后端 AskUserParams）：`questions` 数组优先（多问题分步），
 * 缺省回落单问题 `question` + `options`。解析失败返回空数组（不接管输入框，
 * 工具卡片仍正常渲染——模型可自行处理失败）。
 */
export function parseAskUserQuestions(argumentsRaw: string): {
  question: string;
  options: { label: string; description?: string | null }[];
}[] {
  try {
    const parsed = JSON.parse(argumentsRaw) as {
      questions?: {
        question: string;
        options?: { label: string; description?: string | null }[];
      }[];
      question?: string;
      options?: { label: string; description?: string | null }[];
    };
    if (Array.isArray(parsed.questions) && parsed.questions.length > 0) {
      return parsed.questions.map((q) => ({
        question: q.question ?? '',
        options: q.options ?? [],
      }));
    }
    if (parsed.question) {
      return [{ question: parsed.question, options: parsed.options ?? [] }];
    }
  } catch {
    // 参数解析失败：不接管（工具卡片正常渲染）
  }
  return [];
}

/* ─────── 事件归约器（协议 → store） ─────── */

export interface ChatStreamReducerOptions {
  /** chunk_type=error 时 toast 的兜底文案（服务端未携带 delta 时）。 */
  errorFallbackText?: string;
  /** 流结束（finish_reason 非空 / error）回调：复位流状态 + 注销归约器。 */
  onDone?: (sessionId: string | null) => void;
  /** 首个事件到达时是否 setCurrentSession（新会话 true；已有会话 false——
   * 避免切走后事件把当前会话拉回流式会话）。 */
  adoptOnFirstEvent?: boolean;
}

export interface ChatStreamReducer {
  /** 流式会话归属 id（首个带 session_id 的 chunk 后非空；UI 切走后仍正确）。 */
  readonly streamSessionId: string | null;
  /** 最近完成 chunk 携带的真实上下文窗口（token；历史会话回退用它）。 */
  readonly liveWindow: number;
  /**
   * 归约单个 SSE 事件到 store。
   *
   * 语义与组件解耦：调用方不关心事件如何落到消息结构上；
   * 主对话流与追问流共用同一归约（含 tool_result 挂卡——主对话流此前
   * 未处理该事件导致工具结果丢失，此处为统一时的行为修复）。
   */
  handleEvent(event: ChatStreamEvent): void;
}

/**
 * 创建流式事件归约器。每个流实例一个（跨流状态如 streamSessionId /
 * liveWindow 天然隔离）；事件按会话归属写入 store，切走会话流继续跑，
 * 切回直接显示累积内容。
 */
export function createChatStreamReducer(options: ChatStreamReducerOptions = {}): ChatStreamReducer {
  const onDone = options.onDone;
  const errorFallbackText = options.errorFallbackText ?? '对话处理失败';
  const adoptOnFirstEvent = options.adoptOnFirstEvent ?? true;
  let streamSessionId: string | null = null;

  /** 流结束统一收尾：复位流状态 + 通知调用方（注销归约器）。 */
  const finish = (sid: string | null) => {
    if (sid) useAppStore.getState().setStreamStatus('idle', sid);
    onDone?.(sid);
  };
  let liveWindow = 0;

  return {
    get streamSessionId() {
      return streamSessionId;
    },
    get liveWindow() {
      return liveWindow;
    },

    handleEvent(event) {
      const st = useAppStore.getState();
      const sid = event.session_id || null;

      // 流归属会话：首个带 session_id 的 chunk 建立会话（PENDING 迁移交给 store）。
      // 仅新会话（adoptOnFirstEvent）才 setCurrentSession——已有会话时事件
      // 直接写字典（updateSessionMessages 按 sid 路由），避免切走后被拉回。
      if (sid && sid !== streamSessionId) {
        streamSessionId = sid;
        if (adoptOnFirstEvent) st.setCurrentSession(sid);
      }

      // Server-side error（校验/处理失败）：清理空气泡、复位状态、Toast 提示
      if (event.chunk_type === 'error') {
        st.removeEmptyAssistantMessage();
        finish(sid);
        st.showToast(event.delta || errorFallbackText, 'error');
        return;
      }

      // 用户消息落库确认（ADR-031）：比对 user_message_id 把本地乐观消息
      // 替换为服务端真实 id（回退/重做定位键）；user_message_id 生命周期
      // 到此结束（字段删除，不持久化）。
      if (event.chunk_type === 'user_message_id' && event.user_message_id && event.message_id) {
        st.confirmUserMessageId(sid, event.user_message_id, event.message_id);
        return;
      }

      // 消息边界（统一结构）：流开始/结束携带完整 ChatMessage——本地消息
      // id/内容直接来自服务端结构（与历史加载同构），回退定位键天然正确
      if (event.message) {
        st.applyServerMessage(sid, event.message);
      }

      // 正文增量追加到流归属会话的最后一条 assistant 消息
      if (event.delta) {
        st.updateLastMessage(event.delta, sid);
      }

      // 思考增量单独累积（thinking 字段，折叠展示）。轮次边界：新一轮思考
      // 到达且上一轮已产出内容（正文/工具调用）时新开一条 assistant 消息——
      // 与历史加载「一轮一条消息」的渲染一致；轮次判断读流归属会话的消息
      // （切走后仍正确）。
      if (event.thinking) {
        const msgs = sid ? (st.sessionMessages[sid] ?? []) : st.messages;
        const last = msgs[msgs.length - 1];
        const hasText = last?.segments?.some((s) => s.type === 'text') ?? false;
        const hasPrevTurn =
          last?.role === 'assistant' &&
          (hasText || (last.tool_calls && last.tool_calls.length > 0));
        if (hasPrevTurn) st.startNewAssistantTurn(sid);
        st.appendThinking(event.thinking, sid);
      }

      // 技能调用信息 → 调用卡片
      if (event.skill_calls && event.skill_calls.length > 0) {
        st.appendSkillCalls(event.skill_calls, sid);
      }

      // 结构化工具调用事件 → tool card 渲染（含展示意图）
      if (event.tool_call) {
        st.appendToolCalls([event.tool_call], sid);
        // ask_user：同步工具语义（对齐 DSH）——工具执行挂起等待用户回答，
        // 问题从调用参数解析（arguments JSON），输入框被问题表单接管；
        // 回答提交到 /chat/answer 后工具结果经本流返回，loop 继续。
        if (event.tool_call.name === 'ask_user') {
          const questions = parseAskUserQuestions(event.tool_call.arguments);
          if (questions.length > 0) {
            st.setPendingClarification({ questions });
          }
        }
      }

      // 工具执行结果事件：按 tool_call_id 关联调用卡片，实时填充
      // 耗时/成败/结果（主对话流与追问流一致挂卡）
      if (event.tool_result) {
        st.applyToolResult(event.tool_result, sid);
      }

      // 完成 chunk 携带 token 用量 → 附加到流归属会话的 assistant 消息；
      // 同时记录真实窗口（供上下文圆环回退展示）
      if (event.usage) {
        liveWindow = event.usage.context_window;
        const { prompt_tokens, completion_tokens, total_tokens, cache_read, cache_write } =
          event.usage;
        st.attachLastMessageUsage(
          { prompt_tokens, completion_tokens, total_tokens, cache_read, cache_write },
          sid,
        );
      }

      // 输出达到 token 上限（finish_reason === 'length'）：标记截断提示
      if (event.finish_reason === 'length') {
        st.markLastMessageTruncated(sid);
        finish(sid);
      }

      // 流式中断（finish_reason === 'interrupted'）：保留部分输出并提示，
      // 不误报为 token 上限截断。toast 只停留 3 秒，必须同时给消息打
      // 内联标记（气泡下方持久的红字提示）。
      if (event.finish_reason === 'interrupted') {
        st.markLastMessageInterrupted(sid);
        st.showToast('流式中断，已保留部分输出', 'error');
        finish(sid);
      }

      // 正常完成（finish_reason === 'stop' 等）：复位流状态 + 注销归约器
      if (event.finish_reason && event.finish_reason !== 'length' && event.finish_reason !== 'interrupted') {
        finish(sid);
      }
    },
  };
}

/* ─────── 活跃流归约器注册表（ADR-028 第 3 步） ─────── */

/**
 * 活跃流归约器注册表：POST /chat/stream 启动后按 session_id 注册，
 * 统一事件通道（GET /events）的 chat_stream 事件按 session_id 路由到
 * 对应归约器。流结束（finish_reason/error）时注销。
 *
 * 并行流式（切走会话后另一会话发新流）互不干扰：每个会话一个归约器。
 */
const activeStreamReducers = new Map<string, ChatStreamReducer>();

/** 注册会话的活跃流归约器（startStream 拿到 session_id 后调用）。 */
export function registerStreamReducer(sessionId: string, reducer: ChatStreamReducer): void {
  activeStreamReducers.set(sessionId, reducer);
}

/** 注销会话的活跃流归约器（流结束/错误时调用）。 */
export function unregisterStreamReducer(sessionId: string): void {
  activeStreamReducers.delete(sessionId);
}

/** 路由 chat_stream 事件到对应会话的归约器（useUnifiedEvents 调用）。 */
export function routeChatStreamEvent(event: ChatStreamEvent): void {
  const sid = event.session_id;
  if (!sid) return;
  const reducer = activeStreamReducers.get(sid);
  if (reducer) reducer.handleEvent(event);
}
