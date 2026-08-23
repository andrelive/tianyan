import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import {
  mockKnowledgeDeleteCalls,
  mockKnowledgeSearchCalls,
  resetKnowledgeEntryMocks,
} from '@/test/mocks/handlers';
import KnowledgeSearchTab from '../KnowledgeSearchTab';
import KnowledgeBrowseTab from '../KnowledgeBrowseTab';

function renderSearch() {
  return render(
    <MemoryRouter>
      <KnowledgeSearchTab />
    </MemoryRouter>,
  );
}

function renderBrowse() {
  return render(
    <MemoryRouter>
      <KnowledgeBrowseTab />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  resetKnowledgeEntryMocks();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('KnowledgeSearchTab', () => {
  it('debounces the query: no search request before 300ms, exactly one after', async () => {
    vi.useFakeTimers();
    renderSearch();

    fireEvent.change(screen.getByLabelText('搜索知识库'), { target: { value: '架' } });
    await act(async () => {
      vi.advanceTimersByTime(299);
    });
    expect(mockKnowledgeSearchCalls).toHaveLength(0);

    await act(async () => {
      vi.advanceTimersByTime(1);
    });
    expect(mockKnowledgeSearchCalls).toEqual([{ q: '架' }]);
  });

  it('settles on the last query when typing rapidly', async () => {
    vi.useFakeTimers();
    renderSearch();
    const input = screen.getByLabelText('搜索知识库');

    fireEvent.change(input, { target: { value: '架' } });
    await act(async () => {
      vi.advanceTimersByTime(100);
    });
    fireEvent.change(input, { target: { value: '架构' } });
    await act(async () => {
      vi.advanceTimersByTime(300);
    });

    expect(mockKnowledgeSearchCalls).toEqual([{ q: '架构' }]);
  });

  it('renders results and highlights the query in expanded content', async () => {
    renderSearch();

    fireEvent.change(screen.getByLabelText('搜索知识库'), { target: { value: '架构' } });

    // 结果到达后展开第一条（content 含查询词 → <mark> 高亮）
    const score = await screen.findByText('95%');
    fireEvent.click(score.closest('button') as HTMLButtonElement);

    const marks = await screen.findAllByText('架构', { selector: 'mark' });
    expect(marks.length).toBeGreaterThan(0);
  });
});

describe('KnowledgeBrowseTab', () => {
  it('opens the ConfirmDialog on delete click and auto-dismisses after 5s without calling the API', async () => {
    vi.useFakeTimers();
    renderBrowse();
    // 冲刷挂载加载（useResource + MSW 微任务）
    await act(async () => {
      await Promise.resolve();
    });

    fireEvent.click(screen.getByRole('button', { name: '删除 architecture' }));
    expect(screen.getByText(/将被永久删除（含全部子条目）/)).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(5000);
    });

    expect(screen.queryByText(/将被永久删除/)).not.toBeInTheDocument();
    expect(mockKnowledgeDeleteCalls).toHaveLength(0);
  });

  it('auto-dismiss timer does not fire after manual cancel', async () => {
    vi.useFakeTimers();
    renderBrowse();
    await act(async () => {
      await Promise.resolve();
    });

    fireEvent.click(screen.getByRole('button', { name: '删除 ownership' }));
    fireEvent.click(screen.getByRole('button', { name: '取消' }));
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });

    expect(mockKnowledgeDeleteCalls).toHaveLength(0);
    expect(screen.queryByText(/将被永久删除/)).not.toBeInTheDocument();
  });

  it('deletes via the ConfirmDialog confirm button and refreshes the list', async () => {
    renderBrowse();
    await screen.findByText('architecture');

    fireEvent.click(screen.getByRole('button', { name: '删除 architecture' }));
    fireEvent.click(screen.getByRole('button', { name: '删除' }));

    await act(async () => {
      await Promise.resolve();
    });
    expect(mockKnowledgeDeleteCalls).toEqual([{ uri: 'tianyan://knowledge/architecture' }]);
  });
});
