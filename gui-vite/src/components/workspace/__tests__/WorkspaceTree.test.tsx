import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { resetWorkspaceMocks, mockWorkspaceTreeCalls } from '@/test/mocks/handlers';
import WorkspaceTree from '../WorkspaceTree';

const onSelectFile = vi.fn();

function renderTree() {
  return render(<WorkspaceTree onSelectFile={onSelectFile} />);
}

beforeEach(() => {
  resetWorkspaceMocks();
  onSelectFile.mockClear();
});

describe('WorkspaceTree', () => {
  it('renders root entries from the API', async () => {
    renderTree();

    await waitFor(() => {
      expect(screen.getByText('src')).toBeInTheDocument();
    });
    expect(screen.getByText('Cargo.toml')).toBeInTheDocument();
    expect(screen.getByText('main.rs')).toBeInTheDocument();
    // 根加载调用：无 path，depth=1
    expect(mockWorkspaceTreeCalls).toContainEqual({ depth: 1 });
  });

  it('lazily fetches children when a directory is expanded', async () => {
    const user = userEvent.setup();
    renderTree();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '展开 src' })).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: '展开 src' }));

    await waitFor(() => {
      expect(screen.getByText('lib.rs')).toBeInTheDocument();
    });
    expect(screen.getByText('utils.rs')).toBeInTheDocument();
    // 展开时按相对路径 + depth=1 请求子条目
    expect(mockWorkspaceTreeCalls).toContainEqual({ path: 'src', depth: 1 });
  });

  it('calls onSelectFile when a file is clicked', async () => {
    const user = userEvent.setup();
    renderTree();

    await waitFor(() => {
      expect(screen.getByText('main.rs')).toBeInTheDocument();
    });

    await user.click(screen.getByText('main.rs'));

    expect(onSelectFile).toHaveBeenCalledTimes(1);
    expect(onSelectFile).toHaveBeenCalledWith('main.rs');
  });
});
