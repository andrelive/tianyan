import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import ToolCallCard from '@/components/chat/ToolCallCard';
import type { ToolCallEvent } from '@/lib/types';

const baseEvent: ToolCallEvent = {
  id: 'call-1',
  name: 'read_file',
  arguments: '{"path":"src/main.rs"}',
  presentation: 'read',
};

describe('ToolCallCard', () => {
  it('renders the presentation label and tool name', () => {
    render(<ToolCallCard event={baseEvent} />);
    expect(screen.getByText('读取文件')).toBeInTheDocument();
    expect(screen.getByText('read_file')).toBeInTheDocument();
  });

  it('expands to show formatted arguments', () => {
    render(<ToolCallCard event={baseEvent} />);
    fireEvent.click(screen.getByRole('button', { name: /读取文件 read_file/ }));
    expect(screen.getByText(/"path": "src\/main\.rs"/)).toBeInTheDocument();
  });

  it('shows failure status and error text for failed calls', () => {
    render(
      <ToolCallCard
        event={{ ...baseEvent, success: false, error: '文件不存在', duration_ms: 1234 }}
      />,
    );
    expect(screen.getByText('失败')).toBeInTheDocument();
    expect(screen.getByText('文件不存在')).toBeInTheDocument();
    expect(screen.getByText('1.23s')).toBeInTheDocument();
  });

  it('shows success status with duration', () => {
    render(<ToolCallCard event={{ ...baseEvent, success: true, duration_ms: 200 }} />);
    expect(screen.getByText('成功')).toBeInTheDocument();
    expect(screen.getByText('200ms')).toBeInTheDocument();
  });

  it('expands the result section to show tool output', () => {
    render(<ToolCallCard event={baseEvent} result={'文件内容…'} />);
    fireEvent.click(screen.getByRole('button', { name: /工具结果/ }));
    expect(screen.getByText('文件内容…')).toBeInTheDocument();
  });
});
