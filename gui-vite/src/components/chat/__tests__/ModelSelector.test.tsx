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

/** 同名模型跨 provider fixture（opencode / ollama 都挂载 glm-5.3-flash）。 */
const duplicateModelNameResponse: ModelsResponse = {
  providers: [
    { name: 'opencode', endpoint: 'https://opencode.ai/zen/go/v1', enabled: true, model_count: 1 },
    { name: 'ollama', endpoint: 'https://ollama.com/v1', enabled: true, model_count: 1 },
  ],
  models: [
    { name: 'glm-5.3-flash', provider: 'opencode', capabilities: ['chat'] },
    { name: 'glm-5.3-flash', provider: 'ollama', capabilities: ['chat'] },
  ],
  preferences: {
    chat: { provider: 'opencode', model: 'glm-5.3-flash' },
    embedding: null,
    vision: null,
  },
};

function renderModelSelector(response: ModelsResponse = twoChatModelsResponse) {
  server.use(http.get('/api/v1/config/models', () => HttpResponse.json(response)));
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

    // 2 个 chat 模型渲染在列表框中（各带 provider 标注）
    await waitFor(() => {
      expect(screen.getByRole('combobox', { name: '选择模型' })).not.toBeDisabled();
    });
    await user.click(screen.getByRole('combobox', { name: '选择模型' }));
    const options = screen.getAllByRole('option');
    expect(options).toHaveLength(2);
    expect(options[0]).toHaveTextContent('gpt-4o');
    expect(options[1]).toHaveTextContent('gpt-4o-mini');
    expect(options[0]).toHaveTextContent('openai');

    // 点击第二个模型：本地状态先更新，再向后端发起 switch 请求（携带 provider）
    await user.click(options[1]);

    await waitFor(() => {
      expect(mockSwitchModelCalls).toEqual([
        { model: 'gpt-4o-mini', capability: 'chat', provider: 'openai' },
      ]);
    });
    expect(useAppStore.getState().selectedModel).toEqual({
      provider: 'openai',
      model: 'gpt-4o-mini',
    });
  });

  it('disambiguates duplicate model names across providers (highlight and switch both target the clicked provider)', async () => {
    const user = userEvent.setup();
    renderModelSelector(duplicateModelNameResponse);

    await waitFor(() => {
      expect(screen.getByRole('combobox', { name: '选择模型' })).not.toBeDisabled();
    });
    await user.click(screen.getByRole('combobox', { name: '选择模型' }));

    // 两个同名选项分别带 provider 标注；高亮只落在 selectedModel 指定的 provider 上
    const options = screen.getAllByRole('option');
    expect(options).toHaveLength(2);
    expect(options[0]).toHaveTextContent('opencode');
    expect(options[1]).toHaveTextContent('ollama');
    expect(options[0]).toHaveAttribute('aria-selected', 'true');
    expect(options[1]).toHaveAttribute('aria-selected', 'false');

    // 点击 ollama 项：切换请求必须携带 provider=ollama（精确消歧，而非第一个命中的 opencode）
    await user.click(options[1]);
    await waitFor(() => {
      expect(mockSwitchModelCalls).toEqual([
        { model: 'glm-5.3-flash', capability: 'chat', provider: 'ollama' },
      ]);
    });
    expect(useAppStore.getState().selectedModel).toEqual({
      provider: 'ollama',
      model: 'glm-5.3-flash',
    });

    // 重新打开列表：高亮已翻转（选中 ollama 项，opencode 项不再误亮）
    await user.click(screen.getByRole('combobox', { name: '选择模型' }));
    const reopened = screen.getAllByRole('option');
    expect(reopened[0]).toHaveAttribute('aria-selected', 'false');
    expect(reopened[1]).toHaveAttribute('aria-selected', 'true');
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
    await user.click(screen.getAllByRole('option')[1]);

    // 本地选择不回滚
    await waitFor(() => {
      expect(useAppStore.getState().selectedModel).toEqual({
        provider: 'openai',
        model: 'gpt-4o-mini',
      });
    });

    // 非阻塞错误通过 toast 呈现
    await waitFor(() => {
      const toast = useAppStore.getState().toasts[0];
      expect(toast).toBeDefined();
      expect(toast?.type).toBe('error');
      expect(toast?.message).toContain('切换模型失败');
    });
  });
});
