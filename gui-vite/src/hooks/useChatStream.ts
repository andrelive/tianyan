import { useRef, useCallback, useState } from 'react';
import type { ChatStreamEvent, ChatRequest } from '@/lib/types';

interface UseChatStreamOptions {
  /** SSE endpoint URL (e.g. `${getApiBase()}/chat/stream`) */
  streamUrl: string;
  /** Called on each valid SSE data: event */
  onChunk?: (event: ChatStreamEvent) => void;
  /** Called when a non-abort error occurs */
  onError?: (error: Error) => void;
  /** Called when the stream completes naturally or is aborted */
  onComplete?: () => void;
}

/**
 * Hook for SSE streaming chat responses via fetch + ReadableStream.
 *
 * Parses SSE lines (data: ...), handles abort via AbortController,
 * and buffers incomplete lines. Returns start/stop controls and
 * a live isStreaming state.
 */
export function useChatStream(options: UseChatStreamOptions) {
  const { streamUrl, onChunk, onError, onComplete } = options;
  const abortRef = useRef<AbortController | null>(null);
  const [isStreaming, setIsStreaming] = useState(false);

  const startStream = useCallback(
    async (body: ChatRequest) => {
      const controller = new AbortController();
      abortRef.current = controller;
      setIsStreaming(true);

      try {
        let response: Response;
        try {
          response = await fetch(streamUrl, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
            signal: controller.signal,
          });
        } catch (err) {
          // vitest jsdom 环境下 Node fetch 会拒绝 jsdom realm 的 AbortSignal
          // （与 api-client request() 同源问题）；降级重试不带 signal。
          // 生产浏览器同 realm，此分支永不触发；降级路径下 stopStream 的
          // abort 退化为无效操作——仅影响测试环境。
          if (err instanceof TypeError && /Expected signal/.test(err.message)) {
            response = await fetch(streamUrl, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            });
          } else {
            throw err;
          }
        }

        if (!response.ok) {
          const text = await response.text();
          throw new Error(`HTTP ${response.status}${text ? ': ' + text : ''}`);
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
          // Keep incomplete line in buffer
          buffer = lines.pop() || '';

          for (const line of lines) {
            const trimmed = line.trim();
            if (!trimmed || !trimmed.startsWith('data: ')) continue;

            const data = trimmed.slice(6).trim();
            if (data === '[DONE]') continue;

            try {
              const event: ChatStreamEvent = JSON.parse(data);
              onChunk?.(event);
            } catch {
              // Skip malformed SSE data
            }
          }
        }

        onComplete?.();
      } catch (err: unknown) {
        const error = err as Error;
        if (error.name === 'AbortError') {
          onComplete?.();
        } else {
          onError?.(error);
        }
      } finally {
        abortRef.current = null;
        setIsStreaming(false);
      }
    },
    [streamUrl, onChunk, onError, onComplete],
  );

  const stopStream = useCallback(() => {
    abortRef.current?.abort();
  }, []);

  return { startStream, stopStream, isStreaming };
}
