import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { resetRoleMocks, mockRoleResetCalls, mockRoleDeleteCalls } from '@/test/mocks/handlers';
import RolesPanel from '../RolesPanel';

function renderRolesPanel() {
  return render(
    <MemoryRouter>
      <RolesPanel />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  resetRoleMocks();
});

describe('RolesPanel', () => {
  it('renders role list with source badges and stats from mock data', async () => {
    renderRolesPanel();

    await waitFor(() => {
      expect(screen.getByText('researcher')).toBeInTheDocument();
    });
    expect(screen.getByText('editor')).toBeInTheDocument();
    // 来源徽标：builtin=内置 / learned=学习演化
    expect(screen.getByText('内置')).toBeInTheDocument();
    expect(screen.getByText('学习演化')).toBeInTheDocument();
    // 统计概览（mockRolesStats）
    expect(screen.getByText('12 次')).toBeInTheDocument();
  });

  it('loads role detail on selection (system prompt + tool whitelist)', async () => {
    const user = userEvent.setup();
    renderRolesPanel();

    await waitFor(() => {
      expect(screen.getByText('researcher')).toBeInTheDocument();
    });
    await user.click(screen.getByText('researcher'));

    await waitFor(() => {
      expect(screen.getByText('你是调研员，负责收集并整理资料。')).toBeInTheDocument();
    });
    expect(screen.getByText('web_search')).toBeInTheDocument();
  });

  it('resets learned role to builtin seed only after confirm dialog approval', async () => {
    const user = userEvent.setup();
    renderRolesPanel();

    // editor 为 learned 源（回退按钮可用；builtin v1 角色回退按钮禁用）
    await waitFor(() => {
      expect(screen.getByText('editor')).toBeInTheDocument();
    });
    await user.click(screen.getByText('editor'));
    await waitFor(() => {
      expect(screen.getByText('回退内置种子')).toBeEnabled();
    });

    // 点回退 → 确认对话框出现；取消 → 不发请求
    await user.click(screen.getByRole('button', { name: '回退内置种子' }));
    expect(screen.getByText(/将 editor 回退到内置种子定义/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '取消' }));
    expect(mockRoleResetCalls).toHaveLength(0);

    // 再次点回退 → 确认（对话框按钮精确名 "回退"）→ 请求发出 + 成功消息
    await user.click(screen.getByRole('button', { name: '回退内置种子' }));
    await user.click(screen.getByRole('button', { name: '回退' }));
    await waitFor(() => {
      expect(mockRoleResetCalls).toEqual(['editor']);
    });
    expect(await screen.findByText('已回退内置种子（下次会话边界生效）')).toBeInTheDocument();
  });

  it('deletes role after confirm dialog approval and closes the detail pane', async () => {
    const user = userEvent.setup();
    renderRolesPanel();

    await waitFor(() => {
      expect(screen.getByText('researcher')).toBeInTheDocument();
    });
    await user.click(screen.getByText('researcher'));
    await waitFor(() => {
      expect(screen.getByText('你是调研员，负责收集并整理资料。')).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '退役删除' }));
    expect(screen.getByText(/退役并删除角色 researcher/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '删除' }));
    await waitFor(() => {
      expect(mockRoleDeleteCalls).toEqual(['researcher']);
    });
    // 删除后详情面板关闭（selectedName → null）
    await waitFor(() => {
      expect(screen.queryByText('你是调研员，负责收集并整理资料。')).not.toBeInTheDocument();
    });
  });
});
