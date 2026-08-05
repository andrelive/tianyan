import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import SettingsPanel from '../SettingsPanel';

/** 13 个 tab 的名称（与 SettingsPanel 中 TABS 定义一致）。 */
const TAB_LABELS = [
  '模型服务',
  '数据存储',
  'Agent 行为',
  '人设编辑',
  '安全',
  '日志',
  '记忆',
  '检索',
  '外观',
  '连接',
  'Ollama',
  'MCP',
  '关于',
];

function renderSettingsPanel() {
  return render(
    <MemoryRouter>
      <SettingsPanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('SettingsPanel', () => {
  it('renders the settings panel with the full tab list after config loads', async () => {
    renderSettingsPanel();

    // 配置加载完成后渲染 13 个 tab
    await waitFor(() => {
      expect(screen.getAllByRole('tab')).toHaveLength(13);
    });
    for (const label of TAB_LABELS) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }

    // 默认激活「模型服务」tab，并显示其内容与保存按钮
    expect(screen.getByRole('tab', { name: '模型服务' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '保存设置' })).toBeInTheDocument();
  });

  it('switches content when clicking different tabs', async () => {
    const user = userEvent.setup();
    renderSettingsPanel();
    await waitFor(() => {
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });

    // 点击「数据存储」→ 显示存储配置区域（数据目录等字段 + 保存按钮）
    await user.click(screen.getByRole('tab', { name: '数据存储' }));
    expect(screen.getByText('数据目录')).toBeInTheDocument();
    expect(screen.getByDisplayValue('~/.local/share/tianyan')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '保存设置' })).toBeInTheDocument();

    // 点击「Ollama」→ 显示 Ollama 配置区域，且该 tab 不渲染全局保存按钮
    await user.click(screen.getByRole('tab', { name: 'Ollama' }));
    expect(screen.getByText('Ollama 配置')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '扫描模型' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '保存设置' })).not.toBeInTheDocument();
  });

  it('shows an error state when loading config fails with HTTP 500', async () => {
    server.use(
      http.get('/api/v1/config', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );

    renderSettingsPanel();

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
    expect(screen.getByRole('alert').textContent).toContain('HTTP 500');
    expect(screen.getByRole('button', { name: '重新加载' })).toBeInTheDocument();
    // 错误时不渲染设置内容
    expect(screen.queryByText('默认模型偏好')).not.toBeInTheDocument();
  });

  it('saves the config via PUT /config when clicking 保存设置', async () => {
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
      expect(screen.getByText('默认模型偏好')).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '保存设置' }));

    // PUT body 为 { config: ... } 后端结构，携带完整配置
    await waitFor(() => {
      expect(putBody).not.toBeNull();
    });
    const body = putBody as {
      config: {
        models: { providers: { name: string }[] };
        storage: { data_dir: string };
      };
    };
    expect(body.config.models.providers[0].name).toBe('openai');
    expect(body.config.storage.data_dir).toBe('~/.local/share/tianyan');

    // 保存成功提示
    await waitFor(() => {
      expect(useAppStore.getState().toast).toEqual({ message: '设置已保存', type: 'success' });
    });
  });
});
