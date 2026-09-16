/**
 * 会话流式事件处理 —— 统一事件通道（GET /events）事件 → store 动作的唯一定义点。
 *
 * 纯函数（无状态、无实例、无注册表）：`handleChatStreamEvent` 把每个
 * chat_stream 事件翻译为 store 动作（含会话归属、轮次边界、截断/中断语义、
 * usage 归位、工具结果挂卡）。
 *
 * 历史演进：早期每个 SSE 响应一条独立连接，需要 per-stream 归约器实例
 * （createChatStreamReducer + 活跃流注册表）区分归属。ADR-028/031 收敛为
 * 统一事件通道后，事件自带 session_id、流实例不再存在——注册表机制成为
 * 遗留负担（唤醒轮事件到达时若无可路由归约器即静默丢弃）。重构为订阅级
 * 常驻：事件 → store 纯函数，不依赖任何注册/注销生命周期。
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

/* ─────── 事件 → store 纯函数（订阅级常驻，无生命周期） ─────── */

/**
 * 处理单个 chat_stream 事件（统一事件通道到达即调用，常驻有效）。
 *
 * 事件自带 session_id（主会话 / 子代理 task_id 同构），直接路由到对应
 * 会话的 store 字典——唤醒轮 / 主对话流 / 追问流全部经此入口，无实例
 * 注册表可丢事件。
 */
export function handleChatStreamEvent(event: ChatStreamEvent): void {
  const st = useAppStore.getState();
  const sid = event.session_id || null;

  // Server-side error（校验/处理失败）：清理空气泡、复位状态、Toast 提示
  if (event.chunk_type === 'error') {
    st.removeEmptyAssistantMessage();
    finish(sid);
    st.showToast(event.delta || '对话处理失败', 'error');
    return;
  }

  // 用户消息落库确认（ADR-031）：比对 user_message_id 把本地乐观消息
  // 替换为服务端真实 id（回退/重做定位键）；user_message_id 生命周期
  // 到此结束（字段删除，不持久化）。
  if (event.chunk_type === 'user_message_id' && event.user_message_id && event.message_id) {
    st.confirmUserMessageId(sid, event.user_message_id, event.message_id);
    return;
  }
  // 轮状态（ADR-035 §9，U10）：**后端轮状态是权威**——驱动输入区联动
  // （auto 轮显示"停止"、用户轮禁用发送）。与 streamStatus 独立：后者由
  // 用户发消息置位，覆盖不了唤醒轮（U10 症状一的根因）。
  if (event.chunk_type === 'turn_state' && event.turn_state && sid) {
    st.setTurnState(sid, {
      state: event.turn_state.state === 'running' ? 'running' : 'idle',
      auto: event.turn_state.auto,
    });
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
  // （切换会话后仍正确）。最后一条不是 assistant（system 通知 / user
  // 消息）时也必须新开——否则 appendThinking 落到上一条 assistant
  // （通知之前的消息），思考显示在通知上方（用户报告的"思考插到通知
  // 前面"根因）。
  if (event.thinking) {
    const msgs = sid ? (st.sessionMessages[sid] ?? []) : st.messages;
    const last = msgs[msgs.length - 1];
    const hasText = last?.segments?.some((s) => s.type === 'text') ?? false;
    const hasPrevTurn =
      last?.role === 'assistant' && (hasText || (last.tool_calls && last.tool_calls.length > 0));
    const lastIsAssistant = last?.role === 'assistant';
    if (hasPrevTurn || !lastIsAssistant) st.startNewAssistantTurn(sid);
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
      // U7：追问绑定来源会话（跨会话隔离）——气泡只在对应会话显示，
      // 回答也提交回该会话；无会话归属的事件不弹（保守）。
      if (questions.length > 0 && sid) {
        st.setPendingClarification({ sessionId: sid, questions });
      }
    }
  }

  // 工具执行结果事件：按 tool_call_id 关联调用卡片，实时填充
  // 耗时/成败/结果（主对话流与追问流一致挂卡）
  if (event.tool_result) {
    st.applyToolResult(event.tool_result, sid);
  }

  // 完成 chunk 携带 token 用量 → 附加到流归属会话的 assistant 消息
  // （上下文圆环经 lastMessageUsage 读取，跨会话独立）
  if (event.usage) {
    const { prompt_tokens, completion_tokens, total_tokens, cache_read, cache_write } = event.usage;
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

  // 正常完成（finish_reason === 'stop' 等）：复位流状态
  // （订阅级常驻：无归约器可注销，事件处理本身无生命周期）
  if (
    event.finish_reason &&
    event.finish_reason !== 'length' &&
    event.finish_reason !== 'interrupted'
  ) {
    finish(sid);
  }
}

/** 流结束统一收尾：复位流状态（事件处理后常驻，仅复位不注销）。 */
function finish(sid: string | null): void {
  if (sid) useAppStore.getState().setStreamStatus('idle', sid);
}
