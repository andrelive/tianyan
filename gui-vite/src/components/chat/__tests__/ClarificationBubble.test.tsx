import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import ClarificationBubble from '@/components/chat/ClarificationBubble';

const QUESTIONS = [
  {
    question: '请确认是否删除该文件？',
    options: [
      { label: '删除', description: '永久移除该文件' },
      { label: '保留', description: '不执行任何操作' },
    ],
  },
];

describe('ClarificationBubble', () => {
  it('renders the question, option rows (label + description) and the custom input', () => {
    render(<ClarificationBubble questions={QUESTIONS} submitting={false} onSubmit={() => {}} />);
    expect(screen.getByText('AI 需要确认')).toBeInTheDocument();
    expect(screen.getByText('请确认是否删除该文件？')).toBeInTheDocument();
    // 选项行：label + description 同处可点行
    expect(screen.getByRole('radio', { name: /删除/ })).toBeInTheDocument();
    expect(screen.getByText('永久移除该文件')).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /保留/ })).toBeInTheDocument();
    expect(screen.getByLabelText('输入对追问的回答')).toBeInTheDocument();
  });

  it('submits the selected option as JSON payload', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<ClarificationBubble questions={QUESTIONS} submitting={false} onSubmit={onSubmit} />);
    await user.click(screen.getByRole('radio', { name: /删除/ }));
    await user.click(screen.getByRole('button', { name: '提交回答' }));
    expect(onSubmit).toHaveBeenCalledTimes(1);
    const payload = JSON.parse(onSubmit.mock.calls[0][0] as string);
    expect(payload.answers[0]).toEqual({ question: '请确认是否删除该文件？', answer: '删除' });
  });

  it('submits the custom answer on Enter', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<ClarificationBubble questions={QUESTIONS} submitting={false} onSubmit={onSubmit} />);
    const input = screen.getByLabelText('输入对追问的回答');
    await user.type(input, '再想想');
    await user.keyboard('{Enter}');
    const payload = JSON.parse(onSubmit.mock.calls[0][0] as string);
    expect(payload.answers[0].answer).toBe('再想想');
  });

  it('custom input replaces the selected option', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<ClarificationBubble questions={QUESTIONS} submitting={false} onSubmit={onSubmit} />);
    await user.click(screen.getByRole('radio', { name: /删除/ }));
    const input = screen.getByLabelText('输入对追问的回答');
    await user.type(input, '再想想');
    await user.click(screen.getByRole('button', { name: '提交回答' }));
    const payload = JSON.parse(onSubmit.mock.calls[0][0] as string);
    expect(payload.answers[0].answer).toBe('再想想');
  });

  it('does not submit while a question is unanswered', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<ClarificationBubble questions={QUESTIONS} submitting={false} onSubmit={onSubmit} />);
    await user.keyboard('{Enter}');
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it('switches to the extra tab and includes extra in the payload', async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(<ClarificationBubble questions={QUESTIONS} submitting={false} onSubmit={onSubmit} />);
    await user.click(screen.getByRole('tab', { name: '补充信息' }));
    await user.type(screen.getByLabelText('补充信息'), '顺便补充一点背景');
    await user.click(screen.getByRole('tab', { name: '问题 1' }));
    await user.click(screen.getByRole('radio', { name: /删除/ }));
    await user.click(screen.getByRole('button', { name: '提交回答' }));
    const payload = JSON.parse(onSubmit.mock.calls[0][0] as string);
    expect(payload.extra).toBe('顺便补充一点背景');
  });

  it('disables input and submit while submitting', () => {
    const onSubmit = vi.fn();
    render(<ClarificationBubble questions={QUESTIONS} submitting onSubmit={onSubmit} />);
    expect(screen.getByLabelText('输入对追问的回答')).toBeDisabled();
    expect(screen.getByRole('button', { name: /提交回答/ })).toBeDisabled();
  });
});
