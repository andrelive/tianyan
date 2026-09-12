import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import ChatPanel from '@/components/chat/ChatPanel';
import type { ChatMessage } from '@/lib/types';

function renderChatPanel(route = '/chat') {
  return render(
    <MemoryRouter initialEntries={[route]}>
      <Routes>
        <Route path="/chat" element={<ChatPanel />} />
        <Route path="/chat/:sessionId" element={<ChatPanel />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('tool call rendering regression', () => {
  it('renders tool cards for history messages with tool_calls', () => {
    // 历史消息携带服务端权威 segments（ADR-019：tool 段 + 结果挂 tool_calls）
    const messages: ChatMessage[] = [
      {
        role: 'user',
        segments: [{ type: 'text', text: '查一下' }],
        timestamp: new Date().toISOString(),
      },
      {
        role: 'assistant',
        timestamp: new Date().toISOString(),
        tool_calls: [
          {
            id: 'call-1',
            name: 'web_search',
            arguments: '{"query":"test"}',
            presentation: 'search',
            result: '{"count":1,"results":[]}',
            success: true,
            duration_ms: 500,
          },
        ],
        segments: [
          {
            type: 'tool',
            tool_call: {
              id: 'call-1',
              name: 'web_search',
              arguments: '{"query":"test"}',
              presentation: 'search',
            },
          },
        ],
      },
    ];
    useAppStore.setState({ messages, streamStatus: {} });
    renderChatPanel();
    // 卡片头应渲染工具名
    expect(screen.getByText('web_search')).toBeInTheDocument();
    // 展开后应显示结果
    // expect(screen.getByText('{"count":1,"results":[]}')).toBeInTheDocument();
  });
});
