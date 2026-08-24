import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import ClarificationBubble from '@/components/chat/ClarificationBubble';

describe('ClarificationBubble', () => {
  it('renders the question, options and the custom input', () => {
    render(
      <ClarificationBubble
        question="请确认是否删除该文件？"
        options={['删除', '保留']}
        submitting={false}
        onSubmit={() => {}}
      />,
    );
    expect(screen.getByText('AI 需要确认')).toBeInTheDocument();
    expect(screen.getByText('请确认是否删除该文件？')).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: '删除' })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: '保留' })).toBeInTheDocument();
    expect(screen.getByLabelText('输入对追问的回答')).toBeInTheDocument();
  });

  it('submits the selected option', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(
      <ClarificationBubble
        question="q"
        options={['删除', '保留']}
        submitting={false}
        onSubmit={onSubmit}
      />,
    );
    await user.click(screen.getByRole('radio', { name: '删除' }));
    await user.click(screen.getByRole('button', { name: '提交回答' }));
    expect(onSubmit).toHaveBeenCalledWith('删除');
  });

  it('submits the custom answer on Enter and clears the input', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(
      <ClarificationBubble question="q" options={[]} submitting={false} onSubmit={onSubmit} />,
    );
    const input = screen.getByLabelText('输入对追问的回答');
    await user.type(input, '  确认删除  ');
    await user.keyboard('{Enter}');
    expect(onSubmit).toHaveBeenCalledWith('确认删除');
    expect(input).toHaveValue('');
  });

  it('custom input replaces the selected option', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(
      <ClarificationBubble
        question="q"
        options={['删除', '保留']}
        submitting={false}
        onSubmit={onSubmit}
      />,
    );
    await user.click(screen.getByRole('radio', { name: '删除' }));
    const input = screen.getByLabelText('输入对追问的回答');
    await user.type(input, '再想想');
    await user.click(screen.getByRole('button', { name: '提交回答' }));
    expect(onSubmit).toHaveBeenCalledWith('再想想');
  });

  it('does not submit empty answers', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(
      <ClarificationBubble question="q" options={[]} submitting={false} onSubmit={onSubmit} />,
    );
    await user.keyboard('{Enter}');
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it('disables input and submit while submitting', () => {
    const onSubmit = vi.fn();
    render(<ClarificationBubble question="q" options={[]} submitting onSubmit={onSubmit} />);
    expect(screen.getByLabelText('输入对追问的回答')).toBeDisabled();
    expect(screen.getByRole('button', { name: /提交回答/ })).toBeDisabled();
  });
});
