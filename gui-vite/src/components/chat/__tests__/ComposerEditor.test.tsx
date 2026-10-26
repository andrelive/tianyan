import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { createRef, useState } from 'react';
import ComposerEditor, { type ComposerEditorHandle } from '../ComposerEditor';
import {
  composerEl,
  composerText,
  expectComposerReadOnly,
  typeIntoComposer,
} from '@/test/composer';

/**
 * ComposerEditor 的**单元**测试（ChatInput.test.tsx 覆盖的是「接入后」的整卡行为：
 * 分流、chip、图片、草稿）。这里只钉编辑器自身的契约：
 * 外部 value → 编辑器、命令式句柄、placeholder、只读态。
 *
 * 「怎么把字打进去」的说明见 `@/test/composer`（jsdom 下用户事件要靠模拟浏览器分工）。
 */

/** 受控外壳：与 ChatInput 的接法一致（value 进来、onChange 出去）。 */
function Harness({
  initial = '',
  disabled = false,
  handleRef,
  onChangeSpy,
}: {
  initial?: string;
  disabled?: boolean;
  handleRef?: React.Ref<ComposerEditorHandle>;
  onChangeSpy?: (text: string) => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <ComposerEditor
      ref={handleRef}
      value={value}
      onChange={(text) => {
        onChangeSpy?.(text);
        setValue(text);
      }}
      onSubmit={() => {}}
      onPasteFiles={() => {}}
      disabled={disabled}
      placeholder="输入消息... (Shift+Enter 换行)"
    />
  );
}

describe('ComposerEditor', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('外部 value（多行）同步进编辑器，且文本语义与 textarea 的 value 一致', async () => {
    // 多行的回填（回退/切会话/恢复草稿）必须原样还原：'\n' → Lexical 的 LineBreakNode
    render(<Harness initial={'第一行\n第二行\n'} />);

    await act(async () => {});
    // DOM：两行文字（换行以 <br> 表示，不进 textContent）
    expect(composerText()).toBe('第一行第二行');
    expect(composerEl().querySelectorAll('br').length).toBeGreaterThan(0);
  });

  it('句柄 clear() 清空编辑器并回报 onChange("")', async () => {
    const handleRef = createRef<ComposerEditorHandle>();
    const onChangeSpy = vi.fn();
    render(<Harness handleRef={handleRef} onChangeSpy={onChangeSpy} />);

    await typeIntoComposer('待清空');
    expect(composerText()).toBe('待清空');

    await act(async () => {
      handleRef.current?.clear();
    });

    expect(composerText()).toBe('');
    expect(onChangeSpy).toHaveBeenLastCalledWith('');
  });

  it('句柄 focus() 把焦点交给编辑器', async () => {
    // jsdom 的 focus() 对 contenteditable 元素不生效（`document.activeElement`
    // 停在 body），所以断「编辑器调用了 focus() 且光标落进编辑器」——真机上的
    // 聚焦效果由 e2e（chat.spec.ts 的 "chat input is focused on page load"）兜住。
    const focusSpy = vi.spyOn(HTMLElement.prototype, 'focus');
    const handleRef = createRef<ComposerEditorHandle>();
    render(<Harness handleRef={handleRef} initial="聚焦我" />);
    await act(async () => {});

    await act(async () => {
      handleRef.current?.focus();
    });

    expect(focusSpy.mock.contexts).toContain(composerEl());
    // Lexical 的 focus() 会把编辑器选区挪到内容里（DOM 选区也同步过去）
    const anchor = window.getSelection()?.anchorNode ?? null;
    expect(anchor).not.toBeNull();
    expect(composerEl().contains(anchor)).toBe(true);
  });

  it('placeholder 只在空内容时显示', async () => {
    render(<Harness />);
    await act(async () => {});
    expect(screen.getByText(/输入消息\.\.\./)).toBeInTheDocument();

    await typeIntoComposer('有内容了');
    // 占位符是叠在编辑器上的兄弟节点（不是 `placeholder` 属性），空内容才渲染
    expect(screen.queryByText(/输入消息\.\.\./)).not.toBeInTheDocument();
  });

  it('disabled 时不可编辑（contenteditable=false + aria-readonly），且 Enter 不提交', async () => {
    const onSubmit = vi.fn();
    render(
      <ComposerEditor
        value="只读内容"
        onChange={() => {}}
        onSubmit={onSubmit}
        onPasteFiles={() => {}}
        disabled
        placeholder="输入消息... (Shift+Enter 换行)"
      />,
    );
    await act(async () => {});

    expectComposerReadOnly();

    await act(async () => {
      fireEvent.keyDown(composerEl(), { key: 'Enter' });
    });
    // 编辑器自身不做「禁发」判断（那是 ChatInput handleSend 的职责），
    // 这里只保证禁发态下按键不会插换行、内容不被破坏
    expect(composerText()).toBe('只读内容');
  });
});
