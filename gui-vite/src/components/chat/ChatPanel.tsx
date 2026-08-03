import { useRef, useEffect, useCallback } from 'react';
import { useParams } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { apiGet, apiPost, deleteSessionMessage, getApiBase } from '@/lib/api-client';
import { useChatStream } from '@/hooks/useChatStream';
import { MessageSquare, Loader2 } from 'lucide-react';
import ChatInput from './ChatInput';
import MessageBubble from './MessageBubble';
import ModelSelector from './ModelSelector';
import type {
  ChatMessage,
  ChatResponse,
  EditMessageRequest,
  RegenerateRequest,
} from '@/lib/types';

export default function ChatPanel() {
  const { sessionId: urlSessionId } = useParams<{ sessionId: string }>();

  // Store state - individual selectors for minimal re-renders
  const messages = useAppStore((s) => s.messages);
  const streamStatus = useAppStore((s) => s.streamStatus);
  const currentSessionId = useAppStore((s) => s.currentSessionId);
  const setCurrentSession = useAppStore((s) => s.setCurrentSession);
  const addMessage = useAppStore((s) => s.addMessage);
  const setStreamStatus = useAppStore((s) => s.setStreamStatus);
  const editMessage = useAppStore((s) => s.editMessage);
  const deleteMessagesFrom = useAppStore((s) => s.deleteMessagesFrom);
  const setMessages = useAppStore((s) => s.setMessages);

  const scrollRef = useRef<HTMLDivElement>(null);

  // Sync URL sessionId to store on mount / navigation
  useEffect(() => {
    if (urlSessionId && urlSessionId !== currentSessionId) {
      setCurrentSession(urlSessionId);
      setMessages([]);
    }
  }, [urlSessionId, currentSessionId, setCurrentSession, setMessages]);

  // Auto-scroll to bottom when messages change
  useEffect(() => {
    if (scrollRef.current) {
      const el = scrollRef.current;
      // Only auto-scroll if user is near the bottom
      const isNearBottom =
        el.scrollHeight - el.scrollTop - el.clientHeight < 150;
      if (isNearBottom) {
        requestAnimationFrame(() => {
          el.scrollTop = el.scrollHeight;
        });
      }
    }
  }, [messages]);

  // ─── Custom streaming via fetch + ReadableStream ─────────────────

  const { startStream, stopStream } = useChatStream({
    streamUrl: `${getApiBase()}/chat/stream`,
    onChunk: (event) => {
      // Update session_id from server response
      if (event.session_id) {
        useAppStore.getState().setCurrentSession(event.session_id);
      }
      // Append content delta to the last assistant message
      if (event.delta) {
        useAppStore.getState().updateLastMessage(event.delta);
      }
      // Attach skill calls to the current assistant message
      if (event.skill_calls && event.skill_calls.length > 0) {
        useAppStore.getState().appendSkillCalls(event.skill_calls);
      }
    },
    onError: (error) => {
      useAppStore.getState().setStreamStatus('idle');
      useAppStore.getState().showToast(
        `发送失败: ${error.message}`,
        'error',
      );
    },
    onComplete: () => {
      useAppStore.getState().setStreamStatus('idle');
    },
  });

  // ─── Handlers ───────────────────────────────────────────────────

  const handleSend = useCallback(
    async (content: string) => {
      if (streamStatus === 'streaming' || !content.trim()) return;

      const trimmed = content.trim();

      // Add user message
      addMessage({
        role: 'user',
        content: trimmed,
        timestamp: new Date().toISOString(),
      });

      // Add empty assistant placeholder for streaming
      addMessage({
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
      });

      // Build the API payload: all messages except the empty placeholder
      const state = useAppStore.getState();
      const apiMessages = state.messages.slice(0, -1);

      state.setStreamStatus('streaming');
      await startStream({
        session_id: state.currentSessionId,
        messages: apiMessages,
        stream: true,
        temperature: 0.7,
        max_tokens: 2048,
        model: state.selectedModel,
      });
    },
    [streamStatus, addMessage, startStream]
  );

  const handleRegenerate = useCallback(
    async (assistantIndex: number) => {
      if (streamStatus === 'streaming') return;

      const state = useAppStore.getState();
      const sessionId = state.currentSessionId;
      if (!sessionId) {
        state.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }

      // 找到该回复之前的最近用户消息索引（后端按用户消息索引重新生成）
      let userIndex = -1;
      for (let i = assistantIndex - 1; i >= 0; i--) {
        if (state.messages[i]?.role === 'user') {
          userIndex = i;
          break;
        }
      }
      if (userIndex < 0) {
        state.showToast('未找到可重新生成的消息', 'error');
        return;
      }

      // 乐观更新：截断到该用户消息（含），占位等待新回复
      deleteMessagesFrom(userIndex + 1);
      addMessage({
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
      });

      state.setStreamStatus('streaming');
      try {
        const resp = await apiPost<ChatResponse>('/chat/regenerate', {
          session_id: sessionId,
          message_index: userIndex,
        } satisfies RegenerateRequest);
        replaceLastAssistant(resp.message);
      } catch (err: unknown) {
        state.showToast(
          `重新生成失败: ${err instanceof Error ? err.message : '未知错误'}`,
          'error'
        );
        await reloadSession(sessionId);
      } finally {
        state.setStreamStatus('idle');
      }
    },
    [streamStatus, deleteMessagesFrom, addMessage]
  );

  const handleEdit = useCallback(
    async (index: number, newContent: string) => {
      if (streamStatus === 'streaming' || !newContent.trim()) return;

      const trimmed = newContent.trim();
      const state = useAppStore.getState();
      const sessionId = state.currentSessionId;
      if (!sessionId) {
        state.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }

      // 乐观更新：编辑 + 截断 + 占位等待新回复
      editMessage(index, trimmed);
      deleteMessagesFrom(index + 1);
      addMessage({
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
      });

      state.setStreamStatus('streaming');
      try {
        const resp = await apiPost<ChatResponse>('/chat/edit', {
          session_id: sessionId,
          message_index: index,
          new_content: trimmed,
        } satisfies EditMessageRequest);
        replaceLastAssistant(resp.message);
      } catch (err: unknown) {
        state.showToast(
          `编辑失败: ${err instanceof Error ? err.message : '未知错误'}`,
          'error'
        );
        await reloadSession(sessionId);
      } finally {
        state.setStreamStatus('idle');
      }
    },
    [streamStatus, editMessage, deleteMessagesFrom, addMessage]
  );

  const handleDelete = useCallback(
    async (index: number) => {
      if (streamStatus === 'streaming') return;

      const state = useAppStore.getState();
      const sessionId = state.currentSessionId;
      if (!sessionId) {
        state.showToast('请先发送一条消息以创建会话', 'error');
        return;
      }

      // 乐观更新：删除该消息及其后的所有消息
      deleteMessagesFrom(index);
      try {
        const resp = await deleteSessionMessage(sessionId, index);
        state.setMessages(resp.messages);
      } catch (err: unknown) {
        state.showToast(
          `删除失败: ${err instanceof Error ? err.message : '未知错误'}`,
          'error'
        );
        await reloadSession(sessionId);
      }
    },
    [streamStatus, deleteMessagesFrom]
  );

  const handleStop = useCallback(() => {
    stopStream();
    setStreamStatus('idle');
  }, [stopStream, setStreamStatus]);

  // 用服务器返回的回复替换最后一条 assistant 占位消息
  const replaceLastAssistant = useCallback((msg: ChatResponse['message']) => {
    const state = useAppStore.getState();
    const messages = [...state.messages];
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === 'assistant') {
        messages[i] = {
          ...messages[i],
          content: msg.content,
          timestamp: msg.timestamp ?? new Date().toISOString(),
        };
        break;
      }
    }
    state.setMessages(messages);
  }, []);

  // 从后端重新加载会话消息（失败回滚，恢复与持久化一致的状态）
  const reloadSession = useCallback(async (sessionId: string) => {
    try {
      const data = await apiGet<{ messages: ChatMessage[] }>(
        `/sessions/${sessionId}/messages`
      );
      useAppStore.getState().setMessages(data.messages);
    } catch {
      useAppStore.getState().setMessages([]);
    }
  }, []);

  // Determine which message is currently streaming
  const streamingIndex =
    streamStatus === 'streaming'
      ? messages.length - 1
      : -1;

  // ─── Render ────────────────────────────────────────────────────

  return (
    <div className="flex flex-col h-full bg-[var(--color-bg-primary)]">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] shrink-0">
        <h1 className="text-lg font-semibold text-[var(--color-text-primary)]">
          对话
        </h1>
        <ModelSelector />
      </div>

      {/* Messages area */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto px-4 py-4"
      >
        {messages.length === 0 && streamStatus === 'idle' && (
          <div className="flex flex-col items-center justify-center h-full text-[var(--color-text-tertiary)] gap-3">
            <MessageSquare className="w-12 h-12 opacity-30" />
            <p className="text-sm">开始一段新的对话</p>
            <p className="text-xs opacity-60">
              输入消息开始与 AI 助手交流
            </p>
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
                onRegenerate={handleRegenerate}
                onEdit={handleEdit}
                onDelete={handleDelete}
              />
            ))}

            {/* Loading indicator: streaming started but no content yet */}
            {streamStatus === 'streaming' &&
              messages.length > 0 &&
              messages[messages.length - 1].role === 'assistant' &&
              messages[messages.length - 1].content === '' && (
                <div className="flex items-center gap-2 text-[var(--color-text-tertiary)] py-2" aria-live="polite" aria-label="AI 正在思考中">
                  <Loader2 className="w-4 h-4 animate-spin" />
                  <span className="text-sm">思考中...</span>
                </div>
              )}
          </div>
        )}
      </div>

      {/* Input area */}
      <ChatInput
        onSend={handleSend}
        onStop={handleStop}
        isStreaming={streamStatus === 'streaming'}
      />
    </div>
  );
}
