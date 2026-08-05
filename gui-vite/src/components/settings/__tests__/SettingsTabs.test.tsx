import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import {
  mockTianyanConfig,
  mockOllamaScanResult,
  mockOllamaTestResult,
  mockSoulContent,
} from '@/test/mocks/handlers';
import SettingsPanel from '../SettingsPanel';

function renderSettingsPanel() {
  return render(
    <MemoryRouter>
      <SettingsPanel />
    </MemoryRouter>,
  );
}

/** 覆盖 GET /config 为 2 个 provider / 3 个 model 的配置。 */
function useTwoProviderConfig() {
  server.use(
    http.get('/api/v1/config', () => {
      return HttpResponse.json({
        config: {
          ...mockTianyanConfig,
          models: {
            providers: [
              {
                name: 'openai',
                endpoint: 'https://api.openai.com/v1',
                api_key: 'sk-test',
                models: [
                  { name: 'gpt-4o', capabilities: ['chat', 'vision'] },
                  { name: 'text-embedding-3-small', capabilities: ['text-embedding'] },
                ],
                timeout: 60,
                enabled: true,
                headers: {},
              },
              {
                name: 'deepseek',
                endpoint: 'https://api.deepseek.com/v1',
                api_key: 'sk-deepseek',
                models: [{ name: 'deepseek-chat', capabilities: ['chat'] }],
                timeout: 60,
                enabled: true,
                headers: {},
              },
            ],
            preferences: {
              chat: { provider: 'openai', model: 'gpt-4o' },
              embedding: null,
              vision: null,
            },
          },
        },
      });
    }),
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('SettingsPanel tabs', () => {
  it('renders provider and model lists from config (2 providers / 3 models)', async () => {
    useTwoProviderConfig();
    renderSettingsPanel();

    // 模型列表：3 个模型名称 + 2 个 provider 端点
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });
    expect(screen.getByDisplayValue('text-embedding-3-small')).toBeInTheDocument();
    expect(screen.getByDisplayValue('deepseek-chat')).toBeInTheDocument();
    expect(screen.getByDisplayValue('https://api.openai.com/v1')).toBeInTheDocument();
    expect(screen.getByDisplayValue('https://api.deepseek.com/v1')).toBeInTheDocument();

    // 每个 provider 一个「测试连接」按钮
    expect(screen.getAllByRole('button', { name: '测试连接' })).toHaveLength(2);
  });

  it('calls POST /config/test-connection when clicking 测试连接', async () => {
    const user = userEvent.setup();
    let postBody: unknown = null;

    server.use(
      http.post('/api/v1/config/test-connection', async ({ request }) => {
        postBody = await request.json();
        return HttpResponse.json({ success: true, message: '连接成功' });
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '测试连接' })).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '测试连接' }));

    await waitFor(() => {
      expect(postBody).not.toBeNull();
    });
    expect(postBody).toEqual({
      endpoint: 'https://api.openai.com/v1',
      api_key: 'sk-test',
      model: 'gpt-4o',
    });
    await waitFor(() => {
      expect(useAppStore.getState().toast).toEqual({ message: 'openai 连接成功', type: 'success' });
    });
  });

  it('switches the default chat model via preference select and persists it on save', async () => {
    const user = userEvent.setup();
    useTwoProviderConfig();
    let putBody: unknown = null;

    server.use(
      http.put('/api/v1/config', async ({ request }) => {
        putBody = await request.json();
        return HttpResponse.json({ success: true, message: '配置已保存' });
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });

    // 第一个下拉为「对话 (Chat)」偏好，默认 openai/gpt-4o
    const chatSelect = screen.getAllByRole('combobox')[0];
    expect(chatSelect).toHaveValue('openai|gpt-4o');

    // 切换当前模型为 deepseek/deepseek-chat
    await user.selectOptions(chatSelect, 'deepseek|deepseek-chat');
    expect(chatSelect).toHaveValue('deepseek|deepseek-chat');

    // 保存 → PUT /config 携带新的偏好
    await user.click(screen.getByRole('button', { name: '保存设置' }));
    await waitFor(() => {
      expect(putBody).not.toBeNull();
    });
    const body = putBody as {
      config: {
        models: { preferences: { chat: { provider: string; model: string } | null } };
      };
    };
    expect(body.config.models.preferences.chat).toEqual({
      provider: 'deepseek',
      model: 'deepseek-chat',
    });
  });

  it('renders MCP servers and adds a new one via the form', async () => {
    const user = userEvent.setup();
    let postBody: unknown = null;

    server.use(
      http.post('/api/v1/config/mcp/servers', async ({ request }) => {
        postBody = await request.json();
        return HttpResponse.json({ success: true });
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('tab', { name: 'MCP' }));

    // 服务器列表来自 GET /config/mcp/servers
    await waitFor(() => {
      expect(screen.getByText('filesystem-server')).toBeInTheDocument();
    });
    expect(screen.getByText('code-search-server')).toBeInTheDocument();

    // 填写添加表单并提交
    await user.click(screen.getByRole('button', { name: '添加服务器' }));
    await user.type(screen.getByPlaceholderText('例如: filesystem-server'), 'my-server');
    await user.type(screen.getByPlaceholderText('例如: npx'), 'npx');
    await user.type(
      screen.getByPlaceholderText('例如: @anthropic/mcp-serve, start'),
      '-y, @anthropic-ai/mcp-serve',
    );
    await user.type(
      screen.getByPlaceholderText('例如: GITHUB_TOKEN=ghp_xxx, API_KEY=sk-xxx'),
      'GITHUB_TOKEN=ghp_yyy',
    );
    await user.type(screen.getByPlaceholderText('文件系统操作、代码搜索等'), '我的测试服务器');
    await user.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => {
      expect(postBody).not.toBeNull();
    });
    expect(postBody).toEqual({
      name: 'my-server',
      command: 'npx',
      args: ['-y', '@anthropic-ai/mcp-serve'],
      env: { GITHUB_TOKEN: 'ghp_yyy' },
      enabled: true,
      description: '我的测试服务器',
    });

    // 新服务器出现在列表中
    await waitFor(() => {
      expect(screen.getByText('my-server')).toBeInTheDocument();
    });
  });

  it('scans Ollama models via POST /config/ollama/scan and renders the results', async () => {
    const user = userEvent.setup();
    let scanBody: unknown = null;

    server.use(
      http.post('/api/v1/config/ollama/scan', async ({ request }) => {
        scanBody = await request.json();
        return HttpResponse.json(mockOllamaScanResult);
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('tab', { name: 'Ollama' }));

    await user.click(screen.getByRole('button', { name: '扫描模型' }));

    await waitFor(() => {
      expect(scanBody).toEqual({ endpoint: 'http://localhost:11434' });
    });
    await waitFor(() => {
      expect(screen.getByText('llama3:8b')).toBeInTheDocument();
    });
    expect(screen.getByText('nomic-embed-text')).toBeInTheDocument();
    expect(screen.getByText('已发现模型 (2)')).toBeInTheDocument();
    expect(screen.getByText('4.7 GB')).toBeInTheDocument();

    await waitFor(() => {
      expect(useAppStore.getState().toast).toEqual({ message: '找到 2 个模型', type: 'success' });
    });
  });

  it('tests the Ollama connection and shows the connected version', async () => {
    const user = userEvent.setup();
    server.use(
      http.post('/api/v1/config/ollama/test', () => {
        return HttpResponse.json(mockOllamaTestResult);
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('tab', { name: 'Ollama' }));

    await user.click(screen.getByRole('button', { name: '测试连接' }));

    await waitFor(() => {
      expect(screen.getByText('已连接 (v0.3.6)')).toBeInTheDocument();
    });
  });

  it('loads soul content and saves edited content via PUT /config/soul', async () => {
    const user = userEvent.setup();
    let putBody: unknown = null;

    server.use(
      http.put('/api/v1/config/soul', async ({ request }) => {
        putBody = await request.json();
        return HttpResponse.json({ success: true, message: '已保存' });
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('tab', { name: '人设编辑' }));

    // 加载 GET /config/soul 返回的内容
    const textarea = await screen.findByLabelText('智能体人格内容');
    await waitFor(() => {
      expect(textarea).toHaveValue(mockSoulContent.content);
    });

    // 编辑并保存
    await user.clear(textarea);
    await user.type(textarea, '你是天演，请用简洁的语言回答。');
    await user.click(screen.getByRole('button', { name: '保存人格' }));

    await waitFor(() => {
      expect(putBody).toEqual({ content: '你是天演，请用简洁的语言回答。' });
    });
    await waitFor(() => {
      expect(useAppStore.getState().toast).toEqual({
        message: '智能体人格已保存，下次对话生效',
        type: 'success',
      });
    });
  });

  it('shows an error state with retry when MCP servers fail to load', async () => {
    const user = userEvent.setup();
    server.use(
      http.get('/api/v1/config/mcp/servers', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('tab', { name: 'MCP' }));

    await waitFor(() => {
      expect(screen.getByText(/HTTP 500/)).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument();
  });
});
