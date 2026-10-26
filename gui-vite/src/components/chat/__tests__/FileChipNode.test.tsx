import { describe, it, expect } from 'vitest';
import { $createParagraphNode, $createTextNode, $getRoot, createEditor } from 'lexical';
import { serializeInlineChip } from '@/lib/attachments';
import { $createFileChipNode, $isFileChipNode, FileChipNode } from '../FileChipNode';

/**
 * `FileChipNode` 的**序列化契约**测试。
 *
 * 只钉纯模型层（不渲染 React）：chip 的文本形态就是跨层契约——
 * `EditorState.getTextContent()` 出去的是消息内容、回来的是草稿恢复的输入，
 * 任何格式漂移都会让「输入框 chip ↔ 模型看到的文本 ↔ 消息区 chip」三处走形。
 *
 * Lexical 的节点只能在编辑器语境（`editor.update`）里创建，所以统一用
 * `inEditor` 包一层；`discrete: true` 让 update 同步落盘，便于把结果带出来。
 */

function inEditor<T>(fn: () => T): T {
  const editor = createEditor({
    namespace: 'file-chip-node-test',
    nodes: [FileChipNode],
    onError: (error: Error) => {
      throw error;
    },
  });
  let result!: T;
  editor.update(
    () => {
      result = fn();
    },
    { discrete: true },
  );
  return result;
}

describe('FileChipNode / 序列化（唯一跨层契约）', () => {
  it('getTextContent() 精确等于 `@file:"<路径>"`（含反引号）', () => {
    const path = 'F:\\work\\git\\tianyan\\张磊--系统架构-0603-应急项目.docx';
    const text = inEditor(() => $createFileChipNode(path).getTextContent());
    expect(text).toBe('`@file:"F:\\work\\git\\tianyan\\张磊--系统架构-0603-应急项目.docx"`');
    // 与纯函数口径一致（两处必须同一份真相）
    expect(text).toBe(serializeInlineChip(path));
    // 首尾字符必须是反引号：模型侧看到的是 Markdown 行内代码
    expect(text.startsWith('`')).toBe(true);
    expect(text.endsWith('`')).toBe(true);
  });

  it('嵌在段落文字中间时，整段文本是「文字 + 行内 code + 文字」', () => {
    const text = inEditor(() => {
      const paragraph = $createParagraphNode();
      paragraph.append($createTextNode('帮我看下 '));
      paragraph.append($createFileChipNode('C:\\a b\\c.docx'));
      paragraph.append($createTextNode(' 的架构'));
      $getRoot().append(paragraph);
      return $getRoot().getTextContent();
    });
    expect(text).toBe('帮我看下 `@file:"C:\\a b\\c.docx"` 的架构');
    // 换行/块结构没有被 chip 破坏（仍是一个段落）
    expect(text).not.toContain('\n');
  });

  it('getType() / isInline() / isKeyboardSelectable()：行内、可被光标选中', () => {
    const info = inEditor(() => {
      const node = $createFileChipNode('/a/b.ts');
      return {
        type: FileChipNode.getType(),
        inline: node.isInline(),
        keyboardSelectable: node.isKeyboardSelectable(),
        path: node.getPath(),
      };
    });
    expect(info).toEqual({
      type: 'file-chip',
      inline: true,
      keyboardSelectable: true,
      path: '/a/b.ts',
    });
  });

  it('exportJSON / importJSON 往返：路径无损、类型可用于再注册', () => {
    const result = inEditor(() => {
      const original = $createFileChipNode('C:\\Program Files\\demo\\main.ts');
      const json = original.exportJSON();
      const revived = FileChipNode.importJSON(json);
      return {
        json,
        path: revived.getPath(),
        text: revived.getTextContent(),
        isChip: $isFileChipNode(revived),
      };
    });

    expect(result.json).toMatchObject({
      type: 'file-chip',
      version: 1,
      path: 'C:\\Program Files\\demo\\main.ts',
    });
    expect(result.path).toBe('C:\\Program Files\\demo\\main.ts');
    expect(result.text).toBe('`@file:"C:\\Program Files\\demo\\main.ts"`');
    expect(result.isChip).toBe(true);
  });

  it('$isFileChipNode 只认 chip 节点（文字节点不算）', () => {
    const flags = inEditor(() => ({
      chip: $isFileChipNode($createFileChipNode('/a.ts')),
      text: $isFileChipNode($createTextNode('普通文字')),
      nothing: $isFileChipNode(null),
    }));
    expect(flags).toEqual({ chip: true, text: false, nothing: false });
  });
});
