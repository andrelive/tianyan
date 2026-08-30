import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { Rocket, Cpu, Database, Bot } from 'lucide-react';
import { http, HttpResponse } from 'msw';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import ConfigWizard from '../ConfigWizard';
import StepIndicator from '../StepIndicator';

function renderWizard() {
  return render(
    <MemoryRouter>
      <ConfigWizard />
    </MemoryRouter>,
  );
}

async function fillModelStep(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText('提供商名称'), 'test-provider');
  await user.type(screen.getByLabelText('模型名称'), 'test-model');
  await user.type(screen.getByLabelText('端点 URL'), 'http://localhost:9999/v1');
  await user.type(screen.getByLabelText('API Key'), 'sk-test-key');
}

/** 从欢迎页走完各步骤到达确认页（step 5），沿途填写必填项。 */
async function walkToConfirm(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: '下一步' })); // 欢迎 → 模型
  await fillModelStep(user);
  await user.click(screen.getByRole('button', { name: '下一步' })); // 模型 → 数据
  await user.type(screen.getByLabelText('数据目录'), '~/.local/share/tianyan-test');
  await user.click(screen.getByRole('button', { name: '下一步' })); // 数据 → Web
  await user.click(screen.getByRole('button', { name: '下一步' })); // Web → Agent
  await user.click(screen.getByRole('button', { name: '下一步' })); // Agent → 确认
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('ConfigWizard', () => {
  it('renders the welcome step first with step indicator', () => {
    renderWizard();

    expect(screen.getByText('欢迎使用天演')).toBeInTheDocument();
    expect(screen.getByText('1 / 6')).toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: '配置步骤' })).toBeInTheDocument();
    // 欢迎步骤无必填项，下一步可用；没有上一步
    expect(screen.getByRole('button', { name: '下一步' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: '上一步' })).not.toBeInTheDocument();
  });

  it('navigates forward to the model step and back to welcome', async () => {
    const user = userEvent.setup();
    renderWizard();

    await user.click(screen.getByRole('button', { name: '下一步' }));
    expect(screen.getByText('模型服务配置')).toBeInTheDocument();
    expect(screen.getByText('2 / 6')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '上一步' }));
    expect(screen.getByText('欢迎使用天演')).toBeInTheDocument();
    expect(screen.getByText('1 / 6')).toBeInTheDocument();
  });

  it('blocks advancing while required model fields are missing', async () => {
    const user = userEvent.setup();
    renderWizard();

    await user.click(screen.getByRole('button', { name: '下一步' }));
    expect(screen.getByText('模型服务配置')).toBeInTheDocument();

    // 空表单：下一步禁用，点击不推进
    const nextButton = screen.getByRole('button', { name: '下一步' });
    expect(nextButton).toBeDisabled();
    await user.click(nextButton);
    expect(screen.getByText('模型服务配置')).toBeInTheDocument();

    // 只填一个字段仍不满足必填校验
    await user.type(screen.getByLabelText('提供商名称'), 'test-provider');
    expect(screen.getByRole('button', { name: '下一步' })).toBeDisabled();

    // 填满必填字段后放行
    await user.type(screen.getByLabelText('模型名称'), 'test-model');
    await user.type(screen.getByLabelText('端点 URL'), 'http://localhost:9999/v1');
    await user.type(screen.getByLabelText('API Key'), 'sk-test-key');
    const enabledNext = screen.getByRole('button', { name: '下一步' });
    expect(enabledNext).toBeEnabled();

    await user.click(enabledNext);
    expect(screen.getByText('数据存储配置')).toBeInTheDocument();
  });

  it('runs the full flow and submits config via PUT /config', async () => {
    const user = userEvent.setup();
    let putBody: unknown = null;

    server.use(
      http.put('/api/v1/config', async ({ request }) => {
        putBody = await request.json();
        return HttpResponse.json({ success: true, message: '配置已保存' });
      }),
    );

    renderWizard();
    await walkToConfirm(user);

    // 确认页展示汇总
    expect(screen.getByText('确认配置')).toBeInTheDocument();
    expect(screen.getByText('6 / 6')).toBeInTheDocument();
    expect(screen.getByText('test-provider')).toBeInTheDocument();
    expect(screen.getByText('http://localhost:9999/v1')).toBeInTheDocument();
    expect(screen.getByText('sk-test-...')).toBeInTheDocument();
    expect(screen.getByText('~/.local/share/tianyan-test')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '完成配置' }));

    // 提交 payload 形状正确
    await waitFor(() => {
      expect(putBody).not.toBeNull();
    });
    expect(putBody).toMatchObject({
      config: {
        models: {
          providers: [
            {
              name: 'test-provider',
              endpoint: 'http://localhost:9999/v1',
              api_key: 'sk-test-key',
              models: [{ name: 'test-model', capabilities: ['chat'] }],
            },
          ],
          preferences: { chat: { provider: 'test-provider', model: 'test-model' } },
        },
        storage: {
          data_dir: '~/.local/share/tianyan-test',
          vector: { vector_dimension: 1536 },
        },
        agent: { max_turns: 200 },
      },
    });

    // 成功：标记已配置 + 成功 toast
    await waitFor(() => {
      expect(useAppStore.getState().configured).toBe(true);
    });
    expect(useAppStore.getState().toasts[0]).toMatchObject({
      message: '配置完成！正在启动天演...',
      type: 'success',
    });
  });

  it('shows an error toast and keeps the wizard open when submit fails', async () => {
    const user = userEvent.setup();

    server.use(http.put('/api/v1/config', () => new HttpResponse(null, { status: 500 })));

    renderWizard();
    await walkToConfirm(user);

    await user.click(screen.getByRole('button', { name: '完成配置' }));

    await waitFor(() => {
      expect(useAppStore.getState().toasts[0]?.type).toBe('error');
    });
    expect(useAppStore.getState().toasts[0]?.message).toContain('配置保存失败');
    // 向导未关闭：仍在确认页，完成按钮可重试，未标记已配置
    expect(screen.getByText('6 / 6')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '完成配置' })).toBeEnabled();
    expect(useAppStore.getState().configured).not.toBe(true);
  });
});

describe('StepIndicator', () => {
  const steps = [
    { id: 'welcome', label: '欢迎', icon: Rocket },
    { id: 'model', label: '模型配置', icon: Cpu },
    { id: 'data', label: '数据存储', icon: Database },
    { id: 'agent', label: 'Agent 行为', icon: Bot },
  ];

  it('highlights the current step and marks completed steps with a check', () => {
    render(<StepIndicator current={2} steps={steps} />);

    expect(screen.getByRole('navigation', { name: '配置步骤' })).toBeInTheDocument();

    // 当前步骤（索引 2）带 aria-current="step"
    expect(screen.getByText('数据存储').closest('[aria-current="step"]')).not.toBeNull();
    expect(screen.getByText('欢迎').closest('[aria-current="step"]')).toBeNull();

    // 已完成步骤（索引 < current）显示对勾图标
    const completed = screen.getByLabelText('步骤 1: 欢迎');
    expect(completed.querySelector('.lucide-check')).not.toBeNull();

    // 当前步骤显示自身图标而非对勾
    const current = screen.getByLabelText('步骤 3: 数据存储');
    expect(current.querySelector('.lucide-check')).toBeNull();
    expect(current.querySelector('.lucide-database')).not.toBeNull();

    // 未完成步骤无对勾
    const upcoming = screen.getByLabelText('步骤 4: Agent 行为');
    expect(upcoming.querySelector('.lucide-check')).toBeNull();
    expect(upcoming.querySelector('.lucide-bot')).not.toBeNull();
  });
});