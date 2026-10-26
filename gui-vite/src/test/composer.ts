import { act, fireEvent, screen } from '@testing-library/react';
import { expect } from 'vitest';
import { useAppStore, PENDING_SESSION_KEY } from '@/lib/store';

/**
 * 聊天输入框（Lexical `contenteditable`）的测试工具。
 *
 * `<textarea>` → `ContentEditable` 之后，组件测试里「把字打进去」的方式必须换：
 * `fireEvent.change` / `userEvent.type` 对 contenteditable 都不生效。查询方式则
 * **不变**——`aria-label="输入消息"` 原样挂到编辑器 div 上，所以
 * `getByLabelText` / `getByRole('textbox')` 在组件测试与 e2e 里照旧可用。
 *
 * 断言口径也有一处不得不换：
 * - `toHaveValue()` 只认表单元素 → 用 `composerText()`（DOM 文本）/ `draftText()`（store）。
 * - `toBeDisabled()` 只认表单元素 → 用 `expectComposerReadOnly()`。
 *
 * 为什么打字要自己模拟「浏览器那一半」见 `insertOneChar` 的注释。
 */

/** 编辑器元素本身（Lexical 的 contenteditable div）。 */
export function composerEl(): HTMLElement {
  return screen.getByLabelText('输入消息');
}

/**
 * 编辑器里**看得见**的文本。
 *
 * 多行注意：Lexical 用 `<br>` 表示换行，它不进 `textContent`——换行断言请用
 * `draftText()`（与发送内容同一口径，'\n'）。
 */
export function composerText(): string {
  return composerEl().textContent ?? '';
}

/** store 里的草稿文本（= 发送时用的内容；换行以 '\n' 表示，与旧 textarea 的 value 同语义）。 */
export function draftText(key: string = PENDING_SESSION_KEY): string {
  return useAppStore.getState().inputDrafts[key]?.text ?? '';
}

/**
 * 断言输入框处于**只读（disabled）态**。
 *
 * `contenteditable` 没有表单的 `disabled` 属性（jest-dom 的 `toBeDisabled` 只认
 * form 元素，对 div 永远返回 false），Lexical 用 `contenteditable="false"` +
 * `aria-readonly="true"` + `tabindex="-1"` 表达同一语义——断这三项与旧断言等价。
 */
export function expectComposerReadOnly() {
  const el = composerEl();
  expect(el).toHaveAttribute('contenteditable', 'false');
  expect(el).toHaveAttribute('aria-readonly', 'true');
  expect(el).toHaveAttribute('tabindex', '-1');
}

/** 断言输入框可输入（可编辑态）。 */
export function expectComposerEditable() {
  const el = composerEl();
  expect(el).toHaveAttribute('contenteditable', 'true');
  expect(el).not.toHaveAttribute('aria-readonly');
}

/** 把 DOM 光标放到编辑器内容末尾（浏览器里由点击/输入法维护，jsdom 必须手工设置）。 */
export function caretToEnd(editor: HTMLElement) {
  const selection = window.getSelection();
  if (!selection) throw new Error('window.getSelection() 不可用');
  const range = document.createRange();
  const last = editor.querySelector('p')?.lastChild ?? editor;
  if (last.nodeType === Node.TEXT_NODE) {
    range.setStart(last, (last as Text).length);
  } else {
    range.selectNodeContents(last);
  }
  range.collapse(false);
  selection.removeAllRanges();
  selection.addRange(range);
}

/**
 * 模拟浏览器「插入一个字符」：追加到末尾文本节点（没有则新建）+ 挪光标 + 派发 `input`。
 *
 * 为什么不是 `userEvent.type`：Lexical 收到 `beforeinput` 后**把插入交还给浏览器**
 * （它只记下 data，等 `input` 回来再吸收 DOM 结果）。`userEvent.type` 在
 * jsdom + Lexical 下实测既不打字也不报错（依赖 jsdom 的 contenteditable 选区实现），
 * 所以单测自己扮演「浏览器那一半」：改 DOM → 挪光标 → 派发 `input`。之后 Lexical
 * 的 input 处理把 DOM 变更吸收成模型状态 → `onChange` → store，与真实浏览器分工一致
 * （真实浏览器只是由它自己完成前两步）。
 */
export function insertOneChar(editor: HTMLElement, ch: string) {
  const paragraph = editor.querySelector('p') ?? editor;
  const last = paragraph.lastChild;
  let target: Text | null = null;
  if (last?.nodeType === Node.TEXT_NODE) {
    target = last as Text;
  } else if (last?.nodeType === Node.ELEMENT_NODE) {
    // Lexical 把文本包在 <span data-lexical-text> 里
    const inner = last.lastChild;
    if (inner?.nodeType === Node.TEXT_NODE) target = inner as Text;
  }
  if (target) target.appendData(ch);
  else paragraph.appendChild(document.createTextNode(ch));
  caretToEnd(editor);
  editor.dispatchEvent(
    new InputEvent('input', { inputType: 'insertText', data: ch, bubbles: true }),
  );
}

/** 往输入框里「打字」（逐字符，等价于用户一个键一个键敲）。 */
export async function typeIntoComposer(text: string) {
  const editor = composerEl();
  await act(async () => {
    editor.focus();
    caretToEnd(editor);
  });
  for (const ch of text) {
    await act(async () => insertOneChar(editor, ch));
  }
}

/** 在输入框上敲 Enter（`shiftKey` 决定「换行」还是「发送」，`isComposing` 模拟输入法组合中）。 */
export async function pressEnter(options: { shiftKey?: boolean; isComposing?: boolean } = {}) {
  await act(async () => {
    fireEvent.keyDown(composerEl(), {
      key: 'Enter',
      shiftKey: options.shiftKey ?? false,
      isComposing: options.isComposing ?? false,
    });
  });
}

/**
 * 在输入框上粘贴一个文件项（jsdom 无真实剪贴板，只能手工构造 payload）。
 *
 * `types` / `files` 必须给：RichTextPlugin 的 PASTE_COMMAND 处理器会先调
 * `eventFiles(event)` 读 `dataTransfer.types`（判断「是文件还是文本」），
 * 真实浏览器的 DataTransfer 一定带这两项，jsdom 里得自己补。
 */
export function pasteFilesOntoComposer(files: Array<{ file: File | null; type?: string }>) {
  const realFiles = files.map(({ file }) => file).filter((file): file is File => file !== null);
  return act(async () => {
    fireEvent.paste(composerEl(), {
      clipboardData: {
        types: ['Files'],
        files: realFiles,
        items: files.map(({ file, type }) => ({
          kind: 'file',
          // 显式 type 优先（剪贴板项的 MIME 不一定等于 File 上的 type）
          type: type ?? file?.type ?? 'application/octet-stream',
          getAsFile: () => file,
        })),
      },
    });
  });
}

/** 在输入框上粘贴纯文本（同上：`types` 声明为 text/plain，不带文件）。 */
export function pasteTextOntoComposer(text: string) {
  return act(async () => {
    fireEvent.paste(composerEl(), {
      clipboardData: {
        types: ['text/plain'],
        files: [],
        items: [{ kind: 'string', type: 'text/plain', getAsFile: () => null }],
        getData: (type: string) => (type === 'text/plain' ? text : ''),
      },
    });
  });
}
