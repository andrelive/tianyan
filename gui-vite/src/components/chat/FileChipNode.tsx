import type { JSX } from 'react';
import {
  $getNodeByKey,
  DecoratorNode,
  type LexicalEditor,
  type LexicalNode,
  type NodeKey,
  type SerializedLexicalNode,
  type Spread,
} from 'lexical';
import { basenameOf, serializeInlineChip } from '@/lib/attachments';
import FileChip from './FileChip';

/**
 * 输入框里的**行内文件 chip**（Lexical `DecoratorNode`）。
 *
 * 用户要的形态是「chip 嵌在句子里」而不是「顶部一排文件行」：
 *
 * ```text
 * 输入框：  帮我看下 [📄 张磊--系统架构.docx] 的架构
 * 序列化：  帮我看下 `@file:"F:\work\…\张磊--系统架构.docx"` 的架构
 * ```
 *
 * ## 为什么继承 DecoratorNode（而不是自定义 TextNode）
 * chip 是一个 React 组件（图标 + 文件名 + × 按钮），`DecoratorNode` 正是
 * Lexical 为「文本流里的 React 片段」准备的扩展点：`isInline()` 让它在段落里
 * 像文字一样排布，`decorate()` 返回的 React 元素由 `RichTextPlugin` 通过 portal
 * 挂进 `createDOM()` 造出来的宿主 span 里。
 *
 * ## 序列化口径（**唯一的跨层契约**）
 * `getTextContent()` 返回 `` `@file:"<路径>"` ``（**含反引号**）。Lexical 的
 * `EditorState.getTextContent()` 会把每个子节点的 `getTextContent()` 拼起来，
 * 于是整段文字天然是「文字 + Markdown 行内代码 + 文字」：
 * - 模型看到的是一条连贯句子，路径直白可读（`read_file` 直接用）；
 * - 消息区渲染时它是行内 code，**不破坏段落/列表/表格的块结构**（这正是放弃
 *   「正文末尾追加若干 `@file:` 独立行」的原因）；
 * - 输入框的受控 `value` 仍是纯文本：草稿恢复/回退只需把文本塞回去，由
 *   `parseInlineChips` 认回来重建 chip。
 *
 * 序列化字符串只在 `@/lib/attachments` 里生成/识别（`serializeInlineChip` /
 * `parseInlineChips` / `inlineChipPathOf`），本文件不自己拼格式，避免三处口径
 * 漂移。
 */

/** JSON 形态（`exportJSON` / `importJSON` 的往返口径）。 */
export type SerializedFileChipNode = Spread<{ path: string }, SerializedLexicalNode>;

export class FileChipNode extends DecoratorNode<JSX.Element> {
  /** 文件绝对路径（展示用文件名由 `basenameOf` 派生，不单独存：路径才是真相）。 */
  __path: string;

  constructor(path: string, key?: NodeKey) {
    super(key);
    this.__path = path;
  }

  static getType(): string {
    return 'file-chip';
  }

  static clone(node: FileChipNode): FileChipNode {
    return new FileChipNode(node.__path, node.__key);
  }

  static importJSON(serialized: SerializedFileChipNode): FileChipNode {
    return new FileChipNode(serialized.path);
  }

  exportJSON(): SerializedFileChipNode {
    return { type: FileChipNode.getType(), version: 1, path: this.__path };
  }

  /** 文件绝对路径（插入去重、外层取用时读它，不碰 `__path` 私有名）。 */
  getPath(): string {
    return this.__path;
  }

  /**
   * 宿主 DOM：一个 inline `<span>`。
   *
   * Lexical 会在这个元素上自动补 `contenteditable="false"` 与
   * `data-lexical-decorator="true"`（浏览器就不会把 chip 的文字当成可编辑正文），
   * 并把 `decorate()` 的 React 元素 portal 进来。这里只保留最小职责——必须是
   * **行内**元素，否则整段文字会断行。
   */
  createDOM(): HTMLElement {
    const span = document.createElement('span');
    span.className = 'inline-block align-middle';
    return span;
  }

  /** 节点不可变（路径一旦插入就不再变），DOM 永远不需要就地更新。 */
  updateDOM(): false {
    return false;
  }

  /**
   * 序列化口径：文本里就是 Markdown 行内代码（见文件头注释）。
   * **改这里等于改前后端的消息格式**，必须与 `serializeInlineChip` 保持一致。
   */
  getTextContent(): string {
    return serializeInlineChip(this.__path);
  }

  /** 行内节点：与文字同处一段，参与自动换行。 */
  isInline(): boolean {
    return true;
  }

  /** 光标可以「站」在 chip 上（方向键走过去时它是一个可删的节点，Backspace 能删）。 */
  isKeyboardSelectable(): boolean {
    return true;
  }

  /**
   * 渲染成可删除的 chip。
   *
   * 删除按钮回调里**按 key 重新取节点**（而不是直接 `this.remove()`）：chip 上
   * 的点击发生在若干次编辑器更新之后，`this` 属于渲染当时的旧 EditorState，直接
   * 在 `this` 上做写操作会打到过期状态（Lexical 会抛「节点不属于当前编辑器状态」
   * 或静默失效）。`editor.update` + `$getNodeByKey` 才是正确姿势。
   */
  decorate(editor: LexicalEditor): JSX.Element {
    const key = this.getKey();
    return (
      <FileChip
        file={{ path: this.__path, name: basenameOf(this.__path) }}
        className="align-middle"
        onRemove={() => {
          editor.update(
            () => {
              const node = $getNodeByKey(key);
              if ($isFileChipNode(node)) node.remove();
            },
            { tag: 'file-chip-remove' },
          );
          // 点 × 后把焦点还给编辑器：删掉 chip 通常还要接着打字，不能让光标
          // 留在按钮上（与 ComposerEditor.focusComposer 同一套两步聚焦）
          editor.getRootElement()?.focus({ preventScroll: true });
          editor.focus();
        }}
      />
    );
  }
}

/** 造一个 chip 节点（必须在 `editor.update` / `editorState.read` 语境里调用）。 */
export function $createFileChipNode(path: string): FileChipNode {
  return new FileChipNode(path);
}

/** 类型守卫（跨模块识别 chip 节点，别用 `instanceof TextNode` 之类猜）。 */
export function $isFileChipNode(node: LexicalNode | null | undefined): node is FileChipNode {
  return node instanceof FileChipNode;
}
