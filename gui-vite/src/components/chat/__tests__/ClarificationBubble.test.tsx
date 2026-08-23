import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import ClarificationBubble from '@/components/chat/ClarificationBubble';

describe('ClarificationBubble', () => {
  it('renders the question and the answer input', () => {
    render(
      <ClarificationBubble
        question="请确认是否删除该文件？"
        submitting={false}
        onSubmit={() => {}}
      />,
    );
    expect(screen.getByText('AI 需要确认')).toBeInTheDocument();
    expect(screen.getByText('请确认是否删除该文件？')).toBeInTheDocument();
    expect(screen.getByLabelText('输入对追问的回答')).toBeInTheDocument();
  });

  it('submits the trimmed answer on Enter and clears the input', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<ClarificationBubble question="q" submitting={false} onSubmit={onSubmit} />);
    const input = screen.getByLabelText('输入对追问的回答');
    await user.type(input, '  确认删除  ');
    await user.keyboard('{Enter}');
    expect(onSubmit).toHaveBeenCalledWith('确认删除');
    expect(input).toHaveValue('');
  });

  it('does not submit empty answers', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<ClarificationBubble question="q" submitting={false} onSubmit={onSubmit} />);
    await user.keyboard('{Enter}');
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it('disables input and submit while submitting', () => {
    const onSubmit = vi.fn();
    render(<ClarificationBubble question="q" submitting onSubmit={onSubmit} />);
    expect(screen.getByLabelText('输入对追问的回答')).toBeDisabled();
    expect(screen.getByRole('button', { name: /提交回答/ })).toBeDisabled();
  });
});
