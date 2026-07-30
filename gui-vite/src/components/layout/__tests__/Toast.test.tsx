import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { useAppStore } from '@/lib/store';
import Toast from '@/components/layout/Toast';

beforeEach(() => {
  vi.useFakeTimers();
  useAppStore.setState(useAppStore.getInitialState());
});

afterEach(() => {
  vi.useRealTimers();
});

function renderToast() {
  return render(<Toast />);
}

describe('Toast', () => {
  it('renders nothing when toast is null', () => {
    // Default store: toast is null
    const { container } = renderToast();
    expect(container.firstChild).toBeNull();
  });

  it('shows message when toast is set', () => {
    useAppStore.setState({
      toast: { message: '操作成功', type: 'success' },
    });
    renderToast();
    expect(screen.getByText('操作成功')).toBeInTheDocument();
  });

  it('renders error variant with correct styling', () => {
    useAppStore.setState({
      toast: { message: '发生错误', type: 'error' },
    });
    renderToast();

    const toastEl = screen.getByText('发生错误').closest('div[class*="rounded-lg"]');
    expect(toastEl).toBeInTheDocument();
    // The error styling includes red-based classes
    expect(toastEl!.className).toContain('red');
  });

  it('renders success variant with correct styling', () => {
    useAppStore.setState({
      toast: { message: '保存成功', type: 'success' },
    });
    renderToast();

    const toastEl = screen.getByText('保存成功').closest('div[class*="rounded-lg"]');
    expect(toastEl).toBeInTheDocument();
    // The success styling includes green-based classes
    expect(toastEl!.className).toContain('green');
  });

  it('renders info variant with correct styling', () => {
    useAppStore.setState({
      toast: { message: '请稍候', type: 'info' },
    });
    renderToast();

    const toastEl = screen.getByText('请稍候').closest('div[class*="rounded-lg"]');
    expect(toastEl).toBeInTheDocument();
    // The info/default styling includes blue-based classes
    expect(toastEl!.className).toContain('blue');
  });

  it('auto-hides after 3 seconds', () => {
    useAppStore.setState({
      toast: { message: '自动消失', type: 'info' },
    });
    renderToast();

    // Toast should be visible initially
    expect(screen.getByText('自动消失')).toBeInTheDocument();
    expect(useAppStore.getState().toast).not.toBeNull();

    // Fast-forward 3 seconds
    act(() => {
      vi.advanceTimersByTime(3000);
    });

    // Toast should be null (hidden)
    expect(useAppStore.getState().toast).toBeNull();
  });

  it('can be dismissed by clicking on the toast', () => {
    useAppStore.setState({
      toast: { message: '可关闭', type: 'info' },
    });
    renderToast();

    expect(screen.getByText('可关闭')).toBeInTheDocument();

    // Click the toast wrapper to dismiss (onClick on the inner div calls hideToast)
    const toastWrapper = screen.getByText('可关闭').closest('[class*="rounded-lg"]')!;
    fireEvent.click(toastWrapper);

    // Toast should be dismissed
    expect(useAppStore.getState().toast).toBeNull();
  });

  it('renders dismiss button', () => {
    useAppStore.setState({
      toast: { message: '带关闭按钮', type: 'info' },
    });
    renderToast();

    // The ✕ close button should be present
    expect(screen.getByText('✕')).toBeInTheDocument();
  });

  it('clears the timer on unmount', () => {
    const clearTimeoutSpy = vi.spyOn(global, 'clearTimeout');
    useAppStore.setState({
      toast: { message: '卸载测试', type: 'info' },
    });
    const { unmount } = renderToast();

    unmount();

    // clearTimeout should have been called (cleanup from useEffect)
    expect(clearTimeoutSpy).toHaveBeenCalled();
    clearTimeoutSpy.mockRestore();
  });
});
