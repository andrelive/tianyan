import { useCallback, useRef, useState } from 'react';
import type { ChatRequest } from '@/lib/types';
import { consumeSseStream, createChatStreamReducer } from '@/lib/chat-stream';
import { fetchWithSignal } from '@/lib/fetch-with-signal';

interface UseChatStreamOptions {
  /** SSE endpoint URL (e.g. `${getApiBase()}/chat/stream`) */
  streamUrl: string;
  /** chunk_type=error 时 toast 的兜底文案（未携带 delta 时）。 */
  errorFallbackText?: string;
  /** 完成 chunk 携带真实上下文窗口时回调（组件用 ref 记录，避免重渲染）。 */
  onUsageWindow?: (windowTokens: number) => void;
  /** 非中止错误：按流归属会话报告（UI 切走后仍正确）。 */
  onError?: (sessionId: string | null, error: Error) => void;
  /** 流自然完成或中止：按流归属会话报告。 */
  onComplete?: (sessionId: string | null) => void;
}

/**
 * 会话流式 hook（薄层）：fetch + AbortController + 单一 SSE 解析器。
 *
 * 协议事件 → store 的归约逻辑不在本 hook——由 `lib/chat-stream` 的
 * `createChatStreamReducer` 承担（主对话流与追问流共用同一归约）。
 * 返回 start/stop 控制与 isStreaming 状态。
 */
export function useChatStream(options: UseChatStreamOptions) {
  const { streamUrl, errorFallbackText, onUsageWindow, onError, onComplete } = options;
  const abortRef = useRef<AbortController | null>(null);
  const [isStreaming, setIsStreaming] = useState(false);

  const startStream = useCallback(
    async (body: ChatRequest) => {
      const reducer = createChatStreamReducer({ errorFallbackText });
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

        await consumeSseStream(response, (event) => {
          reducer.handleEvent(event);
          if (event.usage) onUsageWindow?.(reducer.liveWindow);
        });

        onComplete?.(reducer.streamSessionId);
      } catch (err: unknown) {
        const error = err as Error;
        if (error.name === 'AbortError') {
          onComplete?.(reducer.streamSessionId);
        } else {
          onError?.(reducer.streamSessionId, error);
        }
      } finally {
        abortRef.current = null;
        setIsStreaming(false);
      }
    },
    [streamUrl, errorFallbackText, onUsageWindow, onError, onComplete],
  );

  const stopStream = useCallback(() => {
    abortRef.current?.abort();
  }, []);

  return { startStream, stopStream, isStreaming };
}
