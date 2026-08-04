import { useRef, useEffect, useCallback, useState } from 'react';
import { useParams } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import {
  apiGet,
  clarifyChat,
  deleteSessionMessage,
  getApiBase,
  redoSessionMessage,
} from '@/lib/api-client';
import { useChatStream } from '@/hooks/useChatStream';
import { MessageSquare, Loader2, Undo2 } from 'lucide-react';
import ChatInput from './ChatInput';
import ClarificationBubble from './ClarificationBubble';
import MessageBubble from './MessageBubble';
import ModelSelector from './ModelSelector';
import type { ChatMessage } from '@/lib/types';

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

  // Sync URL sessionId to store on mount / navigation
  useEffect(() => {
    if (urlSessionId && urlSessionId !== currentSessionId) {
      setCurrentSession(urlSessionId);
      setMessages([]);
      setPendingClarification(null);
    }
  }, [urlSessionId, currentSessionId, setCurrentSession, setMessages, setPendingClarification]);

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
      // Update session_id from server response
      if (event.session_id) {
        useAppStore.getState().setCurrentSession(event.session_id);
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
      // Attach skill calls to the current assistant message
      if (event.skill_calls && event.skill_calls.length > 0) {
        useAppStore.getState().appendSkillCalls(event.skill_calls);
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

  // 提交对 Agent 追问的回答，并追加继续处理的结果
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
      setSubmittingClarify(true);
      try {
        const resp = await clarifyChat(sessionId, answer);
        addMessage({
          role: 'assistant',
          content: resp.message.content,
          timestamp: resp.message.timestamp || new Date().toISOString(),
        });
        state.setPendingClarification(null);
      } catch (err) {
        state.showToast(
          `追问回答失败: ${err instanceof Error ? err.message : '未知错误'}`,
          'error',
        );
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

  // Determine which message is currently streaming
  const streamingIndex = streamStatus === 'streaming' ? messages.length - 1 : -1;

  // ─── Render ────────────────────────────────────────────────────

  return (
    <div className="flex flex-col h-full bg-[var(--color-bg-primary)]">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] shrink-0">
        <h1 className="text-lg font-semibold text-[var(--color-text-primary)]">对话</h1>
        <ModelSelector />
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

            {/* Loading indicator: streaming started but no content yet */}
            {streamStatus === 'streaming' &&
              messages.length > 0 &&
              messages[messages.length - 1].role === 'assistant' &&
              messages[messages.length - 1].content === '' && (
                <div
                  className="flex items-center gap-2 text-[var(--color-text-tertiary)] py-2"
                  aria-live="polite"
                  aria-label="AI 正在思考中"
                >
                  <Loader2 className="w-4 h-4 animate-spin" />
                  <span className="text-sm">思考中...</span>
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

      {/* Input area */}
      <ChatInput
        onSend={handleSend}
        onStop={handleStop}
        isStreaming={streamStatus === 'streaming'}
      />
    </div>
  );
}
