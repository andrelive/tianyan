import { describe, it, expect, beforeAll, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { resetWorkspaceMocks, mockWorkspaceReadCalls } from '@/test/mocks/handlers';
import type { WorkspaceReadResponse } from '@/lib/types';
import FileViewer from '../FileViewer';

const API_BASE = '/api/v1';

// jsdom 未实现 Range.getClientRects，CodeMirror 度量行高时会抛错（被 CM 内部捕获，
// 但会刷 stderr）。补齐最小实现以保持测试输出干净。
beforeAll(() => {
  if (typeof Range !== 'undefined' && typeof Range.prototype.getClientRects !== 'function') {
    Range.prototype.getClientRects = function () {
      return [new DOMRect(0, 0, 0, 0)] as unknown as DOMRectList;
    };
  }
});

beforeEach(() => {
  resetWorkspaceMocks();
});

/**
 * 分页 fixture：与后端 read 契约一致 —— content 末尾追加截断提示行（marker），
 * showing.limit 反映该窗口实际返回的真实行数（不含 marker）。
 */
function page(offset: number, lines: string[], truncated: boolean): WorkspaceReadResponse {
  return {
    path: 'C:\\work\\tianyan\\src\\lib.rs',
    content: lines.join('\n'),
    truncated,
    total_lines: 100,
    showing: { offset, limit: Math.max(lines.length - (truncated ? 1 : 0), 0) },
    binary: false,
  };
}

describe('FileViewer', () => {
  it('advances pagination offset by real content lines, ignoring the truncation marker', async () => {
    const user = userEvent.setup();
    server.use(
      http.get(`${API_BASE}/workspace/read`, ({ request }) => {
        const url = new URL(request.url);
        const offsetRaw = url.searchParams.get('offset');
        mockWorkspaceReadCalls.push({
          path: url.searchParams.get('path') ?? '',
          offset: offsetRaw === null ? undefined : Number(offsetRaw),
          limit: Number(url.searchParams.get('limit') ?? 2000),
        });
        if (offsetRaw === null || offsetRaw === '1') {
          // 第一页：2 行真实内容 + 1 行 marker（共 3 个显示行）
          return HttpResponse.json(
            page(
              1,
              [
                '1#3f|fn main() {',
                '2#a1|    println!("hello");',
                '(Showing lines 1-2 of 100. Use offset=3 to continue.)',
              ],
              true,
            ),
          );
        }
        if (offsetRaw === '3') {
          // 第二页：同样 2 行真实内容 + 1 行 marker
          return HttpResponse.json(
            page(
              3,
              [
                '3#7c|}',
                '4#9d|    println!("world");',
                '(Showing lines 3-4 of 100. Use offset=5 to continue.)',
              ],
              true,
            ),
          );
        }
        // 第三页：1 行，不再截断
        return HttpResponse.json(page(5, ['5#e1|fn main2() {'], false));
      }),
    );

    render(<FileViewer path="src/lib.rs" />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '加载更多' })).toBeInTheDocument();
    });

    // 第一次加载更多：offset = showing.offset(1) + 真实行数(2) = 3
    await user.click(screen.getByRole('button', { name: '加载更多' }));
    await waitFor(() => {
      expect(mockWorkspaceReadCalls.length).toBe(2);
    });
    expect(mockWorkspaceReadCalls[1]).toMatchObject({
      path: 'src/lib.rs',
      offset: 3,
      limit: 2000,
    });

    // 第二次加载更多：offset = 3 + 2 = 5（旧实现把 marker 计入行数 → 6，会跳过一行）
    await user.click(screen.getByRole('button', { name: '加载更多' }));
    await waitFor(() => {
      expect(mockWorkspaceReadCalls.length).toBe(3);
    });
    expect(mockWorkspaceReadCalls[2]).toMatchObject({
      path: 'src/lib.rs',
      offset: 5,
      limit: 2000,
    });
  });

  it('enters edit mode, marks dirty on typing, and discard restores the disk content', async () => {
    const user = userEvent.setup();
    render(<FileViewer path="main.rs" />);

    // 读模式：hashline 剥离显示
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /编辑/ })).toBeInTheDocument();
    });

    // 进入编辑模式：编辑按钮消失，保存/放弃出现
    await user.click(screen.getByRole('button', { name: /编辑/ }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '放弃' })).toBeInTheDocument();
    });
    expect(screen.getByText('编辑模式')).toBeInTheDocument();

    // 初始无 dirty
    expect(screen.queryByLabelText('未保存')).not.toBeInTheDocument();

    // 在编辑器中输入 → dirty 出现，保存按钮可用
    const cmContent = screen.getByText(
      (_, element) => element?.classList.contains('cm-content') ?? false,
    );
    await user.click(cmContent);
    await user.keyboard('x');
    await waitFor(() => {
      expect(screen.getByLabelText('未保存')).toBeInTheDocument();
    });
    const saveButton = screen.getByRole('button', { name: '保存' }) as HTMLButtonElement;
    expect(saveButton.disabled).toBe(false);

    // 放弃 → 退出编辑模式，内容恢复磁盘原文，dirty 清除
    await user.click(screen.getByRole('button', { name: '放弃' }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /编辑/ })).toBeInTheDocument();
    });
    expect(screen.queryByLabelText('未保存')).not.toBeInTheDocument();
    await waitFor(() => {
      expect(
        screen.getByText(
          (_, element) =>
            (element?.classList.contains('cm-line') ?? false) &&
            element?.textContent === 'fn main() {',
        ),
      ).toBeInTheDocument();
    });
    // 放弃后不应残留编辑插入的字符
    expect(screen.queryByText((content) => content.includes('xfn main'))).not.toBeInTheDocument();
  });

  it('loads the full content before entering edit mode when the file is truncated', async () => {
    const user = userEvent.setup();
    server.use(
      http.get(`${API_BASE}/workspace/read`, ({ request }) => {
        const url = new URL(request.url);
        const offsetRaw = url.searchParams.get('offset');
        mockWorkspaceReadCalls.push({
          path: url.searchParams.get('path') ?? '',
          offset: offsetRaw === null ? undefined : Number(offsetRaw),
          limit: Number(url.searchParams.get('limit') ?? 2000),
        });
        if (offsetRaw === null || offsetRaw === '1') {
          return HttpResponse.json(
            page(
              1,
              ['1#3f|fn main() {', '(Showing lines 1-1 of 2. Use offset=2 to continue.)'],
              true,
            ),
          );
        }
        return HttpResponse.json(page(2, ['2#a1|    println!("hello");'], false));
      }),
    );

    render(<FileViewer path="src/lib.rs" />);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /编辑/ })).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: /编辑/ }));

    // 编辑模式出现前应已循环拉完剩余页（offset=2），并显示全量内容
    await waitFor(() => {
      expect(screen.getByText('编辑模式')).toBeInTheDocument();
    });
    expect(mockWorkspaceReadCalls).toContainEqual({
      path: 'src/lib.rs',
      offset: 2,
      limit: 2000,
    });
    expect(
      screen.getByText(
        (_, element) =>
          (element?.classList.contains('cm-line') ?? false) &&
          element?.textContent === '    println!("hello");',
      ),
    ).toBeInTheDocument();
  });
});
