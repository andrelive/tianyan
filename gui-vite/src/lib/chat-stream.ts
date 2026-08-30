/**
 * 会话流式客户端 —— SSE 协议事件 → store 动作的唯一定义点。
 *
 * 职责边界（深模块：小接口承载整个流式协议知识）：
 * - `consumeSseStream`：全前端唯一的 SSE 行解析器（data: 行 / [DONE] / 畸形行跳过）。
 * - `createChatStreamReducer`：协议事件归约器，把每个 chunk 翻译为 store 动作
 *   （含会话归属、轮次边界、截断/中断语义、usage 归位、工具结果挂卡）。
 *
 * 主对话流（/chat/stream）与追问流（/chat/clarify/stream）共用同一归约器，
 * 组件只负责「发起流」与「渲染」，不再内联协议知识。
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

/* ─────── SSE 行解析（唯一实现） ─────── */

/**
 * 消费 SSE 响应体：逐行解析 `data: ` 事件并回调；`[DONE]` 与畸形行跳过；
 * 未完整的行保留在缓冲区跨块拼接。流自然结束时 resolve。
 */
export async function consumeSseStream(
  response: Response,
  onEvent: (event: ChatStreamEvent) => void,
): Promise<void> {
  const reader = response.body?.getReader();
  if (!reader) throw new Error('Response body is not readable');

  const decoder = new TextDecoder();
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split('\n');
    // 最后一行可能不完整，留在缓冲区等下一块
    buffer = lines.pop() || '';

    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed || !trimmed.startsWith('data: ')) continue;

      const data = trimmed.slice(6).trim();
      if (data === '[DONE]') continue;

      try {
        onEvent(JSON.parse(data) as ChatStreamEvent);
      } catch {
        // Skip malformed SSE data
      }
    }
  }
}

/* ─────── 事件归约器（协议 → store） ─────── */

export interface ChatStreamReducerOptions {
  /** chunk_type=error 时 toast 的兜底文案（服务端未携带 delta 时）。 */
  errorFallbackText?: string;
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
  const errorFallbackText = options.errorFallbackText ?? '对话处理失败';
  let streamSessionId: string | null = null;
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

      // 流归属会话：首个带 session_id 的 chunk 建立会话（PENDING 迁移交给 store）
      if (sid && sid !== streamSessionId) {
        streamSessionId = sid;
        st.setCurrentSession(sid);
      }

      // Server-side error（校验/处理失败）：清理空气泡、复位状态、Toast 提示
      if (event.chunk_type === 'error') {
        st.removeEmptyAssistantMessage();
        st.setStreamStatus('idle');
        st.showToast(event.delta || errorFallbackText, 'error');
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
        const hasPrevTurn =
          last?.role === 'assistant' &&
          (last.content !== '' || (last.tool_calls && last.tool_calls.length > 0));
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
      }

      // 流式中断（finish_reason === 'interrupted'）：保留部分输出并提示，
      // 不误报为 token 上限截断。toast 只停留 3 秒，必须同时给消息打
      // 内联标记（气泡下方持久的红字提示）。
      if (event.finish_reason === 'interrupted') {
        st.markLastMessageInterrupted(sid);
        st.showToast('流式中断，已保留部分输出', 'error');
      }
    },
  };
}
