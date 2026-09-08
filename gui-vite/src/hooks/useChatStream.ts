import { useCallback, useRef, useState } from 'react';
import { fetchWithSignal } from '@/lib/fetch-with-signal';

interface UseChatStreamOptions {
  /** 对话启动端点 URL（e.g. `${getApiBase()}/chat/stream`）。 */
  streamUrl: string;
  /** 非 2xx 响应：按启动请求上报（无会话归属时 null）。 */
  onError?: (sessionId: string | null, error: Error) => void;
}

/**
 * 对话启动 hook（ADR-028 第 3 步：POST /chat/stream 收敛为开关）。
 *
 * 不再消费 SSE 响应流——服务端校验 + 启动 AgentLoop 后立即返回 JSON
 * （{status: started, session_id}）。流式输出（增量/边界/工具/usage）全部
 * 经 `GET /events` 统一事件通道下发，由纯函数 handleChatStreamEvent
 * （lib/chat-stream）直接映射 store（无归约器注册表——事件自带
 * session_id，常驻可达唤醒轮/子代理事件）。
 *
 * 流状态复位（streaming → idle）由 handleChatStreamEvent 在收到
 * finish_reason 非空事件时驱动（error 分支已有；stop/length/interrupted
 * 分支补上），本 hook 不再承担完成回调。
 */
export function useChatStream(options: UseChatStreamOptions) {
  const { streamUrl, onError } = options;
  const abortRef = useRef<AbortController | null>(null);
  /** 代际计数：每次 startStream 自增。stop 后立即重发时，旧流的迟到
   * onError 会被判定为过期而丢弃，不覆盖新流的 streaming 状态。 */
  const generationRef = useRef(0);
  const [isStreaming, setIsStreaming] = useState(false);

  const startStream = useCallback(
    // body 为端点私有形状（主对话 ChatRequest）——hook 只负责 JSON 序列化
    // 与启动请求，不关心载荷结构
    async (body: object): Promise<string | null> => {
      const myGen = ++generationRef.current;
      const controller = new AbortController();
      abortRef.current = controller;
      setIsStreaming(true);

      try {
        const response = await fetchWithSignal(
          streamUrl,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
          },
          controller.signal,
        );

        if (!response.ok) {
          const text = await response.text();
          throw new Error(`HTTP ${response.status}${text ? ': ' + text : ''}`);
        }

        // 启动成功：返回服务端确认的 session_id（新会话时前端据此迁移 PENDING）
        const data = (await response.json()) as { session_id?: string };
        return data.session_id ?? null;
      } catch (err: unknown) {
        // 过期流（stop 后已发起新流）：丢弃，不触发回调
        if (generationRef.current !== myGen) return null;
        const error = err as Error;
        if (error.name !== 'AbortError') {
          onError?.(null, error);
        }
        return null;
      } finally {
        if (generationRef.current === myGen) {
          abortRef.current = null;
          setIsStreaming(false);
        }
      }
    },
    [streamUrl, onError],
  );

  const stopStream = useCallback(() => {
    abortRef.current?.abort();
  }, []);

  return { startStream, stopStream, isStreaming };
}
