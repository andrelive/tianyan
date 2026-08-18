import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { mockTianyanConfig, mockProviderScanResult, mockSoulContent } from '@/test/mocks/handlers';
import SettingsPanel from '../SettingsPanel';

function renderSettingsPanel() {
  return render(
    <MemoryRouter>
      <SettingsPanel />
    </MemoryRouter>,
  );
}

/** 模型列表默认折叠 → 展开指定 provider 的模型列表（aria-label: name + 展开模型列表）。 */
/** 模型列表默认折叠 → 展开指定 provider 的模型列表（等待渲染后点击）。 */
async function expandModelList(name: string) {
  await waitFor(() => {
    expect(screen.getByRole('button', { name: name + ' 展开模型列表' })).toBeInTheDocument();
  });
  fireEvent.click(screen.getByRole('button', { name: name + ' 展开模型列表' }));
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

/** 覆盖 GET /config：模型带规格字段（context_length / max_output_tokens / max_input_tokens）＋ 后端解析结果 model_specs。 */
function useResolvedSpecConfig() {
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
                  {
                    name: 'gpt-4o',
                    capabilities: ['chat', 'vision'],
                    context_length: 128000,
                    max_output_tokens: 4096,
                  },
                  {
                    name: 'text-embedding-3-small',
                    capabilities: ['text-embedding'],
                    max_input_tokens: 8192,
                  },
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
        // 后端已解析的生效规格（显式 > 内置表 > 默认）；key = "{provider}/{model}"
        model_specs: {
          'openai/gpt-4o': {
            context_length: 1000000,
            max_output_tokens: 32000,
            max_input_tokens: 968000,
          },
          'openai/text-embedding-3-small': {
            context_length: 32768,
            max_output_tokens: 8192,
            max_input_tokens: 8192,
          },
          'deepseek/deepseek-chat': {
            context_length: 64000,
            max_output_tokens: 8000,
            max_input_tokens: 64000,
          },
        },
      });
    }),
  );
}

/** 覆盖 GET /config：模型带规格字段（context_length / max_output_tokens / max_input_tokens）。 */
function useSpecConfig() {
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
                  {
                    name: 'gpt-4o',
                    capabilities: ['chat', 'vision'],
                    context_length: 128000,
                    max_output_tokens: 4096,
                  },
                  {
                    name: 'text-embedding-3-small',
                    capabilities: ['text-embedding'],
                    max_input_tokens: 8192,
                  },
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
    await expandModelList('openai');
    await expandModelList('deepseek');

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

  it('edits working directory in Agent tab and persists it on save', async () => {
    const user = userEvent.setup();
    let putBody: unknown = null;

    server.use(
      http.put('/api/v1/config', async ({ request }) => {
        putBody = await request.json();
        return HttpResponse.json({ success: true, message: '配置已保存' });
      }),
    );

    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByRole('tab', { name: 'Agent 行为' })).toBeInTheDocument();
    });
    await user.click(screen.getByRole('tab', { name: 'Agent 行为' }));
    await waitFor(() => {
      expect(screen.getByLabelText('工作目录')).toBeInTheDocument();
    });

    const wdInput = screen.getByLabelText('工作目录');
    await user.clear(wdInput);
    await user.type(wdInput, 'D:/code/my-project');
    await user.click(screen.getByRole('button', { name: '保存设置' }));

    await waitFor(() => {
      expect(putBody).not.toBeNull();
    });
    const body = putBody as { config: { agent: { working_directory: string | null } } };
    expect(body.config.agent.working_directory).toBe('D:/code/my-project');
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

  it('scans provider models via POST /config/providers/scan and renders the results', async () => {
    const user = userEvent.setup();
    let scanBody: unknown = null;

    server.use(
      http.post('/api/v1/config/providers/scan', async ({ request }) => {
        scanBody = await request.json();
        return HttpResponse.json(mockProviderScanResult);
      }),
    );

    useTwoProviderConfig();
    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });

    // 默认在「模型服务」tab；将第一个 provider 的扫描协议切换为 Ollama 后扫描
    const protocolSelect = screen.getByRole('combobox', { name: 'openai 扫描协议' });
    await user.selectOptions(protocolSelect, 'ollama');
    await user.click(screen.getAllByRole('button', { name: '扫描模型' })[0]);

    await waitFor(() => {
      expect(scanBody).toEqual({
        endpoint: 'https://api.openai.com/v1',
        protocol: 'ollama',
        // scan 携带 provider 名：命中内置目录时后端零网络返回目录模型
        provider: 'openai',
      });
    });
    await waitFor(() => {
      expect(screen.getByText('llama3:8b')).toBeInTheDocument();
    });
    expect(screen.getByText('nomic-embed-text')).toBeInTheDocument();
    expect(screen.getByText(/已发现模型 \(2\)/)).toBeInTheDocument();
    expect(screen.getByText('4.7 GB')).toBeInTheDocument();

    // 批量 adopt：新模型默认勾选 → 点「添加选中 N 个」→ 模型写入对应 provider
    await user.click(screen.getByRole('button', { name: /添加选中/ }));
    await expandModelList('openai');
    expect(screen.getByDisplayValue('llama3:8b')).toBeInTheDocument();
    expect(screen.getByDisplayValue('nomic-embed-text')).toBeInTheDocument();
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

  it('renders model spec fields from config and persists edits via PUT /config', async () => {
    const user = userEvent.setup();
    useSpecConfig();
    let putBody: unknown = null;

    server.use(
      http.put('/api/v1/config', async ({ request }) => {
        putBody = await request.json();
        return HttpResponse.json({ success: true, message: '配置已保存' });
      }),
    );

    renderSettingsPanel();
    await expandModelList('openai');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });

    // 规格字段渲染出后端下发的值（gpt-4o 的上下文长度/最大输出 + embedding 的输入上限）
    const ctxInput = screen.getByDisplayValue('128000');
    expect(ctxInput).toHaveAttribute('aria-label', '上下文长度');
    expect(screen.getByDisplayValue('4096')).toHaveAttribute('aria-label', '最大输出');
    expect(screen.getByDisplayValue('8192')).toHaveAttribute('aria-label', '嵌入输入上限');

    // 修改上下文长度并保存 → PUT body 携带三个规格字段
    await user.clear(ctxInput);
    await user.type(ctxInput, '200000');
    await user.click(screen.getByRole('button', { name: '保存设置' }));

    await waitFor(() => {
      expect(putBody).not.toBeNull();
    });
    const body = putBody as {
      config: { models: { providers: { models: Record<string, unknown>[] }[] } };
    };
    const models = body.config.models.providers[0].models;
    expect(models[0].context_length).toBe(200000);
    expect(models[0].max_output_tokens).toBe(4096);
    expect(models[1].max_input_tokens).toBe(8192);
  });

  it('gates spec inputs by model capability: chat/vision vs embedding', async () => {
    useSpecConfig();
    renderSettingsPanel();
    await expandModelList('openai');
    await expandModelList('deepseek');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });

    // 两个 chat/vision 模型 → 每个显示上下文长度 + 最大输出（共 2 组）
    expect(screen.getAllByLabelText('上下文长度')).toHaveLength(2);
    expect(screen.getAllByLabelText('最大输出')).toHaveLength(2);
    // 仅一个 embedding 模型 → 只有一个嵌入输入上限
    expect(screen.getAllByLabelText('嵌入输入上限')).toHaveLength(1);
  });

  it('shows built-in default placeholders on unconfigured spec inputs', async () => {
    renderSettingsPanel(); // mockTianyanConfig：模型无规格字段
    await expandModelList('openai');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });

    // chat 模型（gpt-4o）→ 上下文长度 + 最大输出 placeholder
    expect(screen.getByPlaceholderText('默认 32768（内置表自动匹配）')).toBeInTheDocument();
    // 最大输出 + 嵌入输入上限均为「默认 8192」
    expect(screen.getAllByPlaceholderText('默认 8192').length).toBeGreaterThanOrEqual(1);
    // 规格输入框初始为空
    expect(screen.getByLabelText('上下文长度')).toHaveValue(null);
  });

  it('renders resolved spec rows with compact number abbreviations from model_specs', async () => {
    useResolvedSpecConfig();
    renderSettingsPanel();
    await expandModelList('openai');
    await expandModelList('deepseek');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });

    // chat/vision 模型（gpt-4o）：1000000 → "1M" 上下文、32000 → "32K" 输出
    expect(screen.getByText(/生效规格：1M 上下文 \/ 32K 输出/)).toBeInTheDocument();
    // embedding 模型：只显示嵌入上限（8192 → "8K"）
    expect(screen.getByText(/生效规格：嵌入上限 8K/)).toBeInTheDocument();
    // 无显式字段模型（deepseek-chat）：64000 → "64K" 上下文、8000 → "8K" 输出
    expect(screen.getByText(/生效规格：64K 上下文 \/ 8K 输出/)).toBeInTheDocument();
  });

  it('shows 自定义 badge for models with explicit fields and 自动匹配 for auto-matched ones', async () => {
    useResolvedSpecConfig();
    renderSettingsPanel();
    await expandModelList('openai');
    await expandModelList('deepseek');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });

    // gpt-4o 有显式 context_length → 自定义；deepseek-chat 无显式字段 → 自动匹配
    expect(screen.getAllByText('自定义')).toHaveLength(2); // gpt-4o + text-embedding-3-small
    expect(screen.getAllByText('自动匹配')).toHaveLength(1); // deepseek-chat
  });

  it('does not render resolved spec rows when response has no model_specs (old backend)', async () => {
    renderSettingsPanel(); // mockTianyanConfig：响应无 model_specs
    await expandModelList('openai');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });

    // 不渲染生效规格行，也不报错
    expect(screen.queryByText(/生效规格/)).not.toBeInTheDocument();
    expect(screen.queryByText('自定义')).not.toBeInTheDocument();
    expect(screen.queryByText('自动匹配')).not.toBeInTheDocument();
  });

  it('gates resolved spec row fields by capability: embedding shows only input limit', async () => {
    useResolvedSpecConfig();
    renderSettingsPanel();
    await expandModelList('openai');
    await expandModelList('deepseek');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });

    // 三条生效规格行：gpt-4o（chat/vision）、text-embedding-3-small（embedding）、deepseek-chat（chat）
    const rows = screen.getAllByText(/生效规格/);
    expect(rows).toHaveLength(3);
    // embedding 行不含 上下文/输出 字段
    expect(screen.getByText(/生效规格：嵌入上限 8K/)).toBeInTheDocument();
    expect(screen.queryByText(/生效规格：32K 上下文/)).not.toBeInTheDocument();
  });

  it('refreshes resolved spec rows after save (bugfix: stale 生效规格 after PUT /config)', async () => {
    const user = userEvent.setup();
    useResolvedSpecConfig();

    // 保存标记：GET 先返回旧解析（32K），PUT 后返回新解析（384K）——模拟后端热重载
    let saved = false;
    server.use(
      http.put('/api/v1/config', () => {
        saved = true;
        return HttpResponse.json({ success: true, message: '配置已保存' });
      }),
      http.get('/api/v1/config', () => {
        const out = saved ? 384000 : 32000;
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
                    {
                      name: 'gpt-4o',
                      capabilities: ['chat', 'vision'],
                      context_length: 1000000,
                      max_output_tokens: out,
                    },
                  ],
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
          model_specs: {
            'openai/gpt-4o': {
              context_length: 1000000,
              max_output_tokens: out,
              max_input_tokens: 968000,
            },
          },
        });
      }),
    );

    renderSettingsPanel();
    await expandModelList('openai');
    await waitFor(() => {
      expect(screen.getByDisplayValue('gpt-4o')).toBeInTheDocument();
    });
    // 初始：后端解析为 32K 输出
    expect(screen.getByText(/生效规格：1M 上下文 \/ 32K 输出/)).toBeInTheDocument();

    // 用户把最大输出改为 384000 并保存
    const outInput = screen.getByLabelText('最大输出');
    await user.clear(outInput);
    await user.type(outInput, '384000');
    await user.click(screen.getByRole('button', { name: '保存设置' }));

    // 保存后生效规格行应刷新为 384K（不再停留在旧快照 32K）
    await waitFor(() => {
      expect(screen.getByText(/生效规格：1M 上下文 \/ 384K 输出/)).toBeInTheDocument();
    });
  });
});
