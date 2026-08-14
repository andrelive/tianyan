import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { useAppStore } from '@/lib/store';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import type { Session } from '@/lib/types';
import SessionList from '../SessionList';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function LocationDisplay() {
  const location = useLocation();
  return <div data-testid="location-display">{location.pathname}</div>;
}

function renderSessionList() {
  return render(
    <MemoryRouter initialEntries={['/chat']}>
      <SessionList />
      <LocationDisplay />
    </MemoryRouter>,
  );
}

/** 覆盖 GET /sessions 返回指定会话（含工作目录绑定）。 */
function mockSessions(sessions: Session[]) {
  server.use(
    http.get('/api/v1/sessions', () => {
      return HttpResponse.json({ sessions, total: sessions.length });
    }),
  );
}

/** 会话 fixture（可带工作目录绑定）。 */
function makeSession(overrides: Partial<Session> & { id: string; title: string }): Session {
  return {
    created_at: '2026-07-20T10:00:00Z',
    updated_at: '2026-07-23T08:00:00Z',
    message_count: 3,
    working_directory: null,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Suite
// ---------------------------------------------------------------------------

describe('SessionList', () => {
  beforeEach(() => {
    useAppStore.setState(useAppStore.getInitialState());
    useAppStore.setState({
      sessions: [],
      currentSessionId: null,
      currentView: 'chat',
      isSidebarOpen: true,
      messages: [],
      toast: null,
    });
  });

  it('shows empty state when sessions=[]', async () => {
    mockSessions([]);
    renderSessionList();
    await waitFor(() => {
      expect(screen.getByText('暂无会话')).toBeInTheDocument();
    });
  });

  it('shows sessions grouped by working directory (default group for unbound)', async () => {
    mockSessions([
      makeSession({ id: 's1', title: '项目A会话', working_directory: 'C:/proj/a' }),
      makeSession({ id: 's2', title: '项目B会话', working_directory: 'C:/proj/b' }),
      makeSession({ id: 's3', title: '未绑定会话' }),
    ]);
    renderSessionList();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '分组 a' })).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: '分组 b' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '分组 默认' })).toBeInTheDocument();

    expect(screen.getByText('项目A会话')).toBeInTheDocument();
    expect(screen.getByText('项目B会话')).toBeInTheDocument();
    expect(screen.getByText('未绑定会话')).toBeInTheDocument();
  });

  it('clicking a session navigates to /chat/{id} and sets current session', async () => {
    const user = userEvent.setup();
    mockSessions([makeSession({ id: 's1', title: '测试会话' })]);
    renderSessionList();

    await waitFor(() => {
      expect(screen.getByText('测试会话')).toBeInTheDocument();
    });
    await user.click(screen.getByText('测试会话'));

    expect(useAppStore.getState().currentSessionId).toBe('s1');
    expect(screen.getByTestId('location-display').textContent).toBe('/chat/s1');
  });

  it('clicking 分组行 ＋ starts a new session bound to that directory (no dialog)', async () => {
    const user = userEvent.setup();
    mockSessions([makeSession({ id: 's1', title: '项目A会话', working_directory: 'C:/proj/a' })]);
    renderSessionList();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '分组 a' })).toBeInTheDocument();
    });

    const groupRow = screen.getByRole('button', { name: '分组 a' });
    fireEvent.mouseEnter(groupRow);
    const addBtn = await screen.findByRole('button', { name: '在 a 新建会话' });
    await user.click(addBtn);

    expect(useAppStore.getState().newSessionWorkspace).toBe('C:/proj/a');
    expect(useAppStore.getState().currentSessionId).toBeNull();
    expect(screen.getByTestId('location-display').textContent).toBe('/chat');

    // 对应分组下出现「新会话」占位条目（用户可感知新会话归属）
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '新会话' })).toBeInTheDocument();
    });
    // 占位在 a 分组下、不在默认组下（默认组无会话且未选中）
    expect(screen.queryByRole('button', { name: '分组 默认' })).not.toBeInTheDocument();
  });

  it('clicking top 新建会话 clears pending workspace, navigates to /chat and shows placeholder', async () => {
    const user = userEvent.setup();
    useAppStore.setState({ newSessionWorkspace: 'C:/old' });
    mockSessions([]);
    renderSessionList();

    await user.click(screen.getByRole('button', { name: '新建会话' }));

    expect(useAppStore.getState().newSessionWorkspace).toBeNull();
    expect(screen.getByTestId('location-display').textContent).toBe('/chat');

    // 无会话 + 待新建 → 渲染临时「默认」分组 + 占位条目
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '分组 默认' })).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: '新会话' })).toBeInTheDocument();
    expect(screen.queryByText('暂无会话')).not.toBeInTheDocument();
  });

  it('新目录 opens the picker; selecting a directory starts a new session bound to it', async () => {
    const user = userEvent.setup();
    mockSessions([]);
    renderSessionList();

    await user.click(screen.getByRole('button', { name: '新目录' }));
    const dialog = screen.getByRole('dialog', { name: '选择目录' });

    await waitFor(() => {
      expect(within(dialog).getByRole('button', { name: '目录 C:\\' })).toBeInTheDocument();
    });
    await user.dblClick(within(dialog).getByRole('button', { name: '目录 C:\\' }));
    const subDir = await within(dialog).findByRole('button', { name: '目录 sub1' });
    await user.click(subDir);
    await user.click(within(dialog).getByRole('button', { name: '确认选择' }));

    expect(useAppStore.getState().newSessionWorkspace).toBe('C:\\sub1');
    expect(screen.getByTestId('location-display').textContent).toBe('/chat');
  });

  it('placeholder disappears once a session is selected', async () => {
    const user = userEvent.setup();
    mockSessions([makeSession({ id: 's1', title: '项目A会话', working_directory: 'C:/proj/a' })]);
    renderSessionList();

    // 先通过分组「＋」发起新建（占位出现）
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '分组 a' })).toBeInTheDocument();
    });
    const groupRow = screen.getByRole('button', { name: '分组 a' });
    fireEvent.mouseEnter(groupRow);
    await user.click(await screen.findByRole('button', { name: '在 a 新建会话' }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '新会话' })).toBeInTheDocument();
    });

    // 选中已有会话 → 占位消失（新会话不再进行中）
    await user.click(screen.getByText('项目A会话'));
    await waitFor(() => {
      expect(screen.queryByRole('button', { name: '新会话' })).not.toBeInTheDocument();
    });
    expect(useAppStore.getState().currentSessionId).toBe('s1');
  });

  it('添加新工作区后左栏立即出现新分组（已有会话时也不覆盖）', async () => {
    const user = userEvent.setup();
    mockSessions([makeSession({ id: 's1', title: '项目A会话', working_directory: 'C:/proj/a' })]);
    renderSessionList();

    // 已有分组可见
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '分组 a' })).toBeInTheDocument();
    });

    // 添加新工作区：目录选择器 → 选目录
    await user.click(screen.getByRole('button', { name: '新目录' }));
    const dialog = screen.getByRole('dialog', { name: '选择目录' });
    await waitFor(() => {
      expect(within(dialog).getByRole('button', { name: '目录 C:\\' })).toBeInTheDocument();
    });
    await user.dblClick(within(dialog).getByRole('button', { name: '目录 C:\\' }));
    const subDir = await within(dialog).findByRole('button', { name: '目录 sub1' });
    await user.click(subDir);
    await user.click(within(dialog).getByRole('button', { name: '确认选择' }));

    // 新分组（sub1）出现 + 占位条目在其下；已有分组 a 仍在
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '分组 sub1' })).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: '新会话' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '分组 a' })).toBeInTheDocument();
    expect(useAppStore.getState().newSessionWorkspace).toBe('C:\\sub1');
  });

  it('文件视图 button navigates to /workspace', async () => {
    const user = userEvent.setup();
    mockSessions([]);
    renderSessionList();

    await user.click(screen.getByRole('button', { name: '文件视图' }));

    expect(screen.getByTestId('location-display').textContent).toBe('/workspace');
  });

  it('clicking a group row collapses its sessions', async () => {
    const user = userEvent.setup();
    mockSessions([makeSession({ id: 's1', title: '项目A会话', working_directory: 'C:/proj/a' })]);
    renderSessionList();

    await waitFor(() => {
      expect(screen.getByText('项目A会话')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: '分组 a' }));

    expect(screen.queryByText('项目A会话')).not.toBeInTheDocument();
  });

  it('hovering a session reveals the delete button', async () => {
    const user = userEvent.setup();
    mockSessions([makeSession({ id: 's1', title: '测试会话' })]);
    renderSessionList();

    await waitFor(() => {
      expect(screen.getByText('测试会话')).toBeInTheDocument();
    });

    const sessionRow = screen.getByText('测试会话').closest('[role="button"]')!;
    await user.hover(sessionRow);

    await waitFor(() => {
      expect(screen.getByTitle('删除会话')).toBeInTheDocument();
    });
  });

  it('clicking delete succeeds and calls removeSession', async () => {
    const user = userEvent.setup();
    server.use(
      http.delete('/api/v1/sessions/:id', () => {
        return HttpResponse.json({ success: true });
      }),
    );
    mockSessions([
      makeSession({ id: 's1', title: '会话一' }),
      makeSession({ id: 's2', title: '会话二' }),
    ]);
    renderSessionList();

    await waitFor(() => {
      expect(screen.getByText('会话一')).toBeInTheDocument();
    });

    const sessionRow = screen.getByText('会话一').closest('[role="button"]')!;
    fireEvent.mouseEnter(sessionRow);
    await waitFor(() => {
      expect(screen.getByTitle('删除会话')).toBeInTheDocument();
    });
    await user.click(screen.getByTitle('删除会话'));

    await waitFor(() => {
      const sessions = useAppStore.getState().sessions;
      expect(sessions).toHaveLength(1);
      expect(sessions[0].id).toBe('s2');
    });
  });

  it('delete failure shows error toast', async () => {
    server.use(
      http.delete('/api/v1/sessions/:id', () => {
        return new HttpResponse(null, { status: 500 });
      }),
    );
    const user = userEvent.setup();
    mockSessions([makeSession({ id: 's1', title: '测试会话' })]);
    renderSessionList();

    await waitFor(() => {
      expect(screen.getByText('测试会话')).toBeInTheDocument();
    });

    const sessionRow = screen.getByText('测试会话').closest('[role="button"]')!;
    fireEvent.mouseEnter(sessionRow);
    await waitFor(() => {
      expect(screen.getByTitle('删除会话')).toBeInTheDocument();
    });
    await user.click(screen.getByTitle('删除会话'));

    await waitFor(() => {
      const toast = useAppStore.getState().toast;
      expect(toast).not.toBeNull();
      expect(toast!.message).toBe('删除会话失败');
      expect(toast!.type).toBe('error');
    });
  });

  describe('rename', () => {
    const renameSession = makeSession({ id: 's1', title: '旧标题' });

    beforeEach(() => {
      mockSessions([renameSession]);
      useAppStore.setState({ sessions: [renameSession] });
    });

    it('double-clicking a title shows an inline editor', async () => {
      const user = userEvent.setup();
      renderSessionList();

      await waitFor(() => {
        expect(screen.getByText('旧标题')).toBeInTheDocument();
      });
      await user.dblClick(screen.getByText('旧标题'));
      expect(screen.getByLabelText('编辑会话标题')).toBeInTheDocument();
    });

    it('submitting a new title updates the store via the title API', async () => {
      const user = userEvent.setup();
      renderSessionList();

      await waitFor(() => {
        expect(screen.getByText('旧标题')).toBeInTheDocument();
      });
      await user.dblClick(screen.getByText('旧标题'));
      const input = screen.getByLabelText('编辑会话标题');
      await user.clear(input);
      await user.type(input, '新标题');
      await user.keyboard('{Enter}');

      await waitFor(() => {
        const sessions = useAppStore.getState().sessions;
        expect(sessions[0].title).toBe('新标题');
      });
      expect(screen.getByText('新标题')).toBeInTheDocument();
    });
  });
});
