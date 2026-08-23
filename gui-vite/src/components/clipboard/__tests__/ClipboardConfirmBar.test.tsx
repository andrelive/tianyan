import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useAppStore } from '@/lib/store';
import { mockClipboardRespondCalls, resetClipboardMocks } from '@/test/mocks/handlers';
import ClipboardConfirmBar from '../ClipboardConfirmBar';

function renderBar() {
  return render(<ClipboardConfirmBar />);
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  resetClipboardMocks();
});

describe('ClipboardConfirmBar', () => {
  it('polls and shows the pending capture with action buttons', async () => {
    renderBar();

    await waitFor(() => {
      expect(screen.getByText(/检测到复制内容/)).toBeInTheDocument();
    });
    expect(screen.getByText('剪贴板捕获内容')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '存入记忆' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '导入知识库' })).toBeInTheDocument();
  });

  it('responds with remember + text and hides the bar', async () => {
    const user = userEvent.setup();
    renderBar();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '存入记忆' })).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: '存入记忆' }));

    await waitFor(() => {
      expect(mockClipboardRespondCalls).toContainEqual({
        action: 'remember',
        text: '剪贴板捕获内容',
      });
    });
    await waitFor(() => {
      expect(screen.queryByText(/检测到复制内容/)).not.toBeInTheDocument();
    });
  });

  it('ignores without sending text and keeps the bar hidden afterwards', async () => {
    const user = userEvent.setup();
    renderBar();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '忽略' })).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: '忽略' }));

    await waitFor(() => {
      expect(mockClipboardRespondCalls).toContainEqual({ action: 'ignore', text: undefined });
    });
    await waitFor(() => {
      expect(screen.queryByText(/检测到复制内容/)).not.toBeInTheDocument();
    });
  });
});
