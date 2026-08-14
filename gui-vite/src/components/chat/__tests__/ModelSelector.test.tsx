import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { useAppStore } from '@/lib/store';
import {
  resetModelSwitchMocks,
  mockSwitchModelCalls,
  mockModelsResponse,
} from '@/test/mocks/handlers';
import type { ModelsResponse } from '@/lib/types';
import ModelSelector from '../ModelSelector';

/** 含 2 个 chat 模型的响应 fixture（覆盖默认 mock 以便断言 2 个选项）。 */
const twoChatModelsResponse: ModelsResponse = {
  providers: mockModelsResponse.providers,
  models: [
    { name: 'gpt-4o', provider: 'openai', capabilities: ['chat', 'vision'] },
    { name: 'gpt-4o-mini', provider: 'openai', capabilities: ['chat'] },
  ],
  preferences: {
    chat: { provider: 'openai', model: 'gpt-4o' },
    embedding: null,
    vision: null,
  },
};

function renderModelSelector() {
  server.use(http.get('/api/v1/config/models', () => HttpResponse.json(twoChatModelsResponse)));
  return render(<ModelSelector />);
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  resetModelSwitchMocks();
});

describe('ModelSelector', () => {
  it('renders models from GET /config/models; clicking an option POSTs to /config/models/switch and updates the store', async () => {
    const user = userEvent.setup();
    renderModelSelector();

    // 2 个 chat 模型渲染在列表框中
    await waitFor(() => {
      expect(screen.getByRole('combobox', { name: '选择模型' })).not.toBeDisabled();
    });
    await user.click(screen.getByRole('combobox', { name: '选择模型' }));
    expect(screen.getByRole('option', { name: 'gpt-4o' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'gpt-4o-mini' })).toBeInTheDocument();

    // 点击第二个模型：本地状态先更新，再向后端发起 switch 请求
    await user.click(screen.getByRole('option', { name: 'gpt-4o-mini' }));

    await waitFor(() => {
      expect(mockSwitchModelCalls).toEqual([{ model: 'gpt-4o-mini', capability: 'chat' }]);
    });
    expect(useAppStore.getState().selectedModel).toBe('gpt-4o-mini');
  });

  it('keeps the local selection and surfaces an error toast when the switch endpoint fails (500)', async () => {
    const user = userEvent.setup();
    server.use(
      http.post('/api/v1/config/models/switch', () => new HttpResponse(null, { status: 500 })),
    );
    renderModelSelector();

    await waitFor(() => {
      expect(screen.getByRole('combobox', { name: '选择模型' })).not.toBeDisabled();
    });
    await user.click(screen.getByRole('combobox', { name: '选择模型' }));
    await user.click(screen.getByRole('option', { name: 'gpt-4o-mini' }));

    // 本地选择不回滚
    await waitFor(() => {
      expect(useAppStore.getState().selectedModel).toBe('gpt-4o-mini');
    });

    // 非阻塞错误通过 toast 呈现
    await waitFor(() => {
      const toast = useAppStore.getState().toast;
      expect(toast).not.toBeNull();
      expect(toast?.type).toBe('error');
      expect(toast?.message).toContain('切换模型失败');
    });
  });
});
