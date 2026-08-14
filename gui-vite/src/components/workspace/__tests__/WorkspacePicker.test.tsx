import { describe, it, expect } from 'vitest';
import { render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import WorkspacePicker from '@/components/workspace/WorkspacePicker';

/**
 * 工作区目录选择器：浏览根（盘符）→ 双击进入二级浏览 → 选中子目录 →
 * 确认选择回调完整路径；「不绑定工作区」回调空串。
 */
describe('WorkspacePicker', () => {
  it('browses into subdirectories and confirms a selection', async () => {
    const user = userEvent.setup();
    const selected: string[] = [];
    render(
      <WorkspacePicker
        open={true}
        currentWorkingDir=""
        onClose={() => {}}
        onSelect={(p) => selected.push(p)}
        clearLabel="不绑定工作区"
      />,
    );
    const dialog = screen.getByRole('dialog', { name: '选择工作目录' });

    // 浏览根：盘符列表
    await waitFor(() => {
      expect(within(dialog).getByRole('button', { name: '目录 C:\\' })).toBeInTheDocument();
    });

    // 双击进入二级浏览（异步加载子目录）
    await user.dblClick(within(dialog).getByRole('button', { name: '目录 C:\\' }));
    const subDir = await within(dialog).findByRole('button', { name: '目录 sub1' });
    await user.click(subDir);
    await user.click(within(dialog).getByRole('button', { name: '确认选择' }));

    expect(selected).toEqual(['C:\\sub1']);
  });

  it('clears via the 不绑定 button (empty path)', async () => {
    const user = userEvent.setup();
    const selected: string[] = [];
    render(
      <WorkspacePicker
        open={true}
        currentWorkingDir=""
        onClose={() => {}}
        onSelect={(p) => selected.push(p)}
        clearLabel="不绑定工作区"
      />,
    );
    const dialog = screen.getByRole('dialog', { name: '选择工作目录' });
    await waitFor(() => {
      expect(within(dialog).getByRole('button', { name: '目录 C:\\' })).toBeInTheDocument();
    });
    await user.click(within(dialog).getByRole('button', { name: '不绑定工作区' }));
    expect(selected).toEqual(['']);
  });
});
