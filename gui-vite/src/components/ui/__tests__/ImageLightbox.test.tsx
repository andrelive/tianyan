import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import ZoomableImage from '../ImageLightbox';

/** 用例数据（内容无关，仅断言交互）。 */
const SRC = 'data:image/png;base64,dGVzdA==';

/**
 * 图片点击放大（会话内图片与输入框预览共用原语）：
 * 缩略图点击 → 全屏 dialog；Escape / 关闭按钮 / 遮罩点击 → 关闭。
 */
describe('ZoomableImage 点击放大', () => {
  it('默认只渲染缩略图，不渲染预览对话框', () => {
    render(<ZoomableImage src={SRC} alt="图片 1" />);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByAltText('图片 1')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '放大查看：图片 1' })).toBeInTheDocument();
  });

  it('点击缩略图打开全屏预览（dialog + 大图 + 关闭按钮）', () => {
    render(<ZoomableImage src={SRC} alt="图片 1" />);
    fireEvent.click(screen.getByRole('button', { name: '放大查看：图片 1' }));

    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.getByRole('dialog')).toHaveAttribute('aria-modal', 'true');
    expect(screen.getByRole('button', { name: '关闭图片预览' })).toBeInTheDocument();
    // 缩略图 + 预览图（alt 相同 → 两张）
    expect(screen.getAllByAltText('图片 1')).toHaveLength(2);
  });

  it('Escape 关闭', () => {
    render(<ZoomableImage src={SRC} alt="图片 2" />);
    fireEvent.click(screen.getByRole('button', { name: '放大查看：图片 2' }));
    expect(screen.getByRole('dialog')).toBeInTheDocument();

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('关闭按钮关闭', () => {
    render(<ZoomableImage src={SRC} alt="图片 3" />);
    fireEvent.click(screen.getByRole('button', { name: '放大查看：图片 3' }));
    fireEvent.click(screen.getByRole('button', { name: '关闭图片预览' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('遮罩点击关闭（点击大图本身不关闭）', () => {
    render(<ZoomableImage src={SRC} alt="图片 4" />);
    fireEvent.click(screen.getByRole('button', { name: '放大查看：图片 4' }));

    // 点击大图区域：保持打开
    fireEvent.click(screen.getAllByAltText('图片 4')[1]);
    expect(screen.getByRole('dialog')).toBeInTheDocument();

    // 点击遮罩：关闭
    fireEvent.click(screen.getByRole('presentation'));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('叠层子控件（输入框「移除图片」）随缩略图渲染，且点击不触发放大', () => {
    const onRemove = vi.fn();
    render(
      <ZoomableImage src={SRC} alt="待发送图片 1">
        <button onClick={onRemove} aria-label="移除图片">
          x
        </button>
      </ZoomableImage>,
    );

    fireEvent.click(screen.getByRole('button', { name: '移除图片' }));
    expect(onRemove).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
