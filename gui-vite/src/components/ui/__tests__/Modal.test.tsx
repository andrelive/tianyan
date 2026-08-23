import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import Modal from '../Modal';

describe('Modal', () => {
  it('renders dialog semantics with children', () => {
    render(
      <Modal onClose={vi.fn()} ariaLabel="测试模态框">
        <p>模态内容</p>
      </Modal>,
    );
    expect(screen.getByRole('dialog', { name: '测试模态框' })).toBeInTheDocument();
    expect(screen.getByText('模态内容')).toBeInTheDocument();
  });

  it('closes on Escape and on overlay click', () => {
    const onClose = vi.fn();
    render(
      <Modal onClose={onClose} ariaLabel="测试">
        <p>内容</p>
      </Modal>,
    );

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('presentation'));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('does not close when clicking inside the panel', () => {
    const onClose = vi.fn();
    render(
      <Modal onClose={onClose} ariaLabel="测试">
        <button type="button">面板内按钮</button>
      </Modal>,
    );
    fireEvent.click(screen.getByRole('button', { name: '面板内按钮' }));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('ignores Escape and overlay clicks while closeDisabled', () => {
    const onClose = vi.fn();
    render(
      <Modal onClose={onClose} ariaLabel="测试" closeDisabled>
        <p>保存中</p>
      </Modal>,
    );

    fireEvent.keyDown(window, { key: 'Escape' });
    fireEvent.click(screen.getByRole('presentation'));
    expect(onClose).not.toHaveBeenCalled();
  });
});
