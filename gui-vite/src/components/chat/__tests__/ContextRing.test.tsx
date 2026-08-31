import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import ContextRing from '../ContextRing';
import type { StreamUsage } from '@/lib/types';

const usage: StreamUsage = {
  prompt_tokens: 6000,
  completion_tokens: 500,
  total_tokens: 6500,
  context_window: 8000,
  cache_read: 3000,
  cache_write: 0,
};

describe('ContextRing', () => {
  it('renders the occupancy percentage and opens the detail panel on click', () => {
    render(<ContextRing usage={usage} onCompress={vi.fn()} />);

    expect(screen.getByText('75%')).toBeInTheDocument();
    fireEvent.click(screen.getByText('75%'));
    // 详情展开：占用 / 缓存命中 / 输出 / 口径说明
    expect(screen.getByText(/6,000 \/ 8,000 \(75%\)/)).toBeInTheDocument();
    expect(screen.getByText(/3,000 token/)).toBeInTheDocument();
    expect(screen.getByText('500 token')).toBeInTheDocument();
    // 口径说明：圆环 = 最近一轮请求，底部小字 = 会话累计
    expect(screen.getByText(/最近一轮请求/)).toBeInTheDocument();
  });

  it('renders an empty ring without usage (en-dash placeholder)', () => {
    render(<ContextRing usage={null} onCompress={vi.fn()} />);
    expect(screen.getByText('–')).toBeInTheDocument();
  });
});
