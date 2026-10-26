import { describe, it, expect } from 'vitest';
import {
  basenameOf,
  fileIconFor,
  inlineChipPathOf,
  parseAttachmentLines,
  parseInlineChips,
  serializeInlineChip,
  toFileRefs,
  type FileRef,
} from '../attachments';

/** Windows 风格路径（含空格，最容易被引号/分隔符写错的形态）。 */
const WIN_REF: FileRef = { path: 'C:\\Program Files\\demo\\main.ts', name: 'main.ts' };
/** POSIX 风格路径（同一个模块不能写死 `\`）。 */
const POSIX_REF: FileRef = { path: '/home/u/repo/src/lib.rs', name: 'lib.rs' };

describe('attachments / basenameOf', () => {
  it('取文件名且兼容两种分隔符', () => {
    expect(basenameOf('C:\\a\\b\\main.ts')).toBe('main.ts');
    expect(basenameOf('/a/b/main.ts')).toBe('main.ts');
    expect(basenameOf('main.ts')).toBe('main.ts');
    expect(basenameOf('   /a/b/c.md  ')).toBe('c.md');
  });

  it('空串与尾分隔符（目录路径）退化为原串，不产出空名', () => {
    expect(basenameOf('')).toBe('');
    expect(basenameOf('   ')).toBe('');
    // 目录路径：拿不到 basename 时退回原串，chip 至少还能看出是哪个目录
    expect(basenameOf('C:\\a\\b\\')).toBe('C:\\a\\b\\');
  });
});

describe('attachments / toFileRefs', () => {
  it('过滤空串并按 path 去重（保留首次出现顺序）', () => {
    const refs = toFileRefs(['', '  ', WIN_REF.path, WIN_REF.path, POSIX_REF.path]);
    expect(refs.map((r) => r.path)).toEqual([WIN_REF.path, POSIX_REF.path]);
    expect(refs.map((r) => r.name)).toEqual(['main.ts', 'lib.rs']);
  });

  it('空数组 → 空数组', () => {
    expect(toFileRefs([])).toEqual([]);
  });
});

describe('attachments / parseAttachmentLines', () => {
  it('提取引用行并保留正文（整行旧格式）', () => {
    const text = `@file:"${WIN_REF.path}"\n@file:"${POSIX_REF.path}"`;
    const parsed = parseAttachmentLines(text);
    expect(parsed.body).toBe('');
    expect(parsed.refs).toEqual([WIN_REF, POSIX_REF]);
  });

  it('含空格路径 + 正文往返一致', () => {
    const content = `看这个文件\n\n@file:"${WIN_REF.path}"`;
    expect(content).toBe('看这个文件\n\n@file:"C:\\Program Files\\demo\\main.ts"');
    const parsed = parseAttachmentLines(content);
    expect(parsed.refs).toEqual([WIN_REF]);
    // 正文尾部最多保留一个换行（空行分隔被吃掉）
    expect(parsed.body).toBe('看这个文件\n');
  });

  it('CRLF 行尾也能识别引用行', () => {
    const parsed = parseAttachmentLines('你好\r\n\r\n@file:"C:\\a b\\c.ts"\r\n');
    expect(parsed.refs).toEqual([{ path: 'C:\\a b\\c.ts', name: 'c.ts' }]);
    // 尾部连续换行收成一个（正文段落结构不变；markdown 渲染时尾换行不可见）
    expect(parsed.body).toBe('你好\n');
  });

  it('无引用行时正文原文不变（含 CRLF 与尾部空白）', () => {
    const text = '普通文本\r\n第二行  \n\n';
    const parsed = parseAttachmentLines(text);
    expect(parsed.refs).toEqual([]);
    expect(parsed.body).toBe(text);
  });

  it('用户手打的行内 @file: 不产生引用（必须整行匹配）', () => {
    const text = '把 @file:"x.ts" 放到这里\n以及 @file:"y.ts"';
    const parsed = parseAttachmentLines(text);
    expect(parsed.refs).toEqual([]);
    expect(parsed.body).toBe(text);
  });

  it('重复引用行去重', () => {
    const text = '正文\n@file:"C:\\a\\b.ts"\n@file:"C:\\a\\b.ts"';
    const parsed = parseAttachmentLines(text);
    expect(parsed.refs).toEqual([{ path: 'C:\\a\\b.ts', name: 'b.ts' }]);
    expect(parsed.body).toBe('正文');
  });

  it('纯附件消息（无正文）→ body 为空串，不留空段落', () => {
    const parsed = parseAttachmentLines('\n\n@file:"C:\\a\\b.ts"');
    expect(parsed.body).toBe('');
    expect(parsed.refs).toHaveLength(1);
  });

  it('引用行夹在正文中间：正文保留其余行与原顺序', () => {
    const parsed = parseAttachmentLines('第一段\n@file:"C:\\a\\b.ts"\n第二段');
    expect(parsed.body).toBe('第一段\n第二段');
    expect(parsed.refs).toHaveLength(1);
  });
});

/**
 * 行内格式（新消息的唯一序列化口径）：chip 就是一段 Markdown 行内代码。
 *
 * 判别力：序列化与解析若不同步（谁改了格式而另一头没跟上），下面每组往返都会
 * 立刻炸——这是「输入框 chip ↔ 模型/消息区文本」之间唯一的契约。
 */
describe('attachments / serializeInlineChip', () => {
  it('输出反引号包裹的 @file:"路径"（行内 code）', () => {
    expect(serializeInlineChip('C:\\a b\\c.ts')).toBe('`@file:"C:\\a b\\c.ts"`');
    expect(serializeInlineChip(POSIX_REF.path)).toBe('`@file:"/home/u/repo/src/lib.rs"`');
  });
});

describe('attachments / inlineChipPathOf', () => {
  it('单个行内 code 文本 → 路径（消息区 code 渲染器用它判 chip）', () => {
    expect(inlineChipPathOf('@file:"C:\\a b\\c.ts"')).toBe('C:\\a b\\c.ts');
    expect(inlineChipPathOf(POSIX_REF.path)).toBe(null);
    // 带反引号的原文不是 code 渲染器看到的形态（children 已去掉反引号）
    expect(inlineChipPathOf(serializeInlineChip('/a/b.ts'))).toBe(null);
    // 多行/夹带其他文字都不算 chip
    expect(inlineChipPathOf('@file:"a.ts"\n')).toBe(null);
    expect(inlineChipPathOf('见 @file:"a.ts"')).toBe(null);
  });
});

describe('attachments / parseInlineChips', () => {
  it('纯文本（无标记）→ 单段且原文原样（含 CRLF 与尾部空白）', () => {
    const text = '普通文本\r\n第二行  \n\n';
    expect(parseInlineChips(text)).toEqual([{ type: 'text', text }]);
  });

  it('空串 → 单个空文本段', () => {
    expect(parseInlineChips('')).toEqual([{ type: 'text', text: '' }]);
  });

  it('chip 在句子中间：切成「文字 + chip + 文字」', () => {
    const text = '看这个 `@file:"C:\\a b\\c.ts"` 文件';
    expect(parseInlineChips(text)).toEqual([
      { type: 'text', text: '看这个 ' },
      { type: 'chip', path: 'C:\\a b\\c.ts' },
      { type: 'text', text: ' 文件' },
    ]);
  });

  it('chip 在开头 / 结尾（不要产出空文本段）', () => {
    expect(parseInlineChips('`@file:"/a.ts"` 后面有字')).toEqual([
      { type: 'chip', path: '/a.ts' },
      { type: 'text', text: ' 后面有字' },
    ]);
    expect(parseInlineChips('前面有字 `@file:"/a.ts"`')).toEqual([
      { type: 'text', text: '前面有字 ' },
      { type: 'chip', path: '/a.ts' },
    ]);
    expect(parseInlineChips('`@file:"/a.ts"`')).toEqual([{ type: 'chip', path: '/a.ts' }]);
  });

  it('一行里多个 chip + 多行混排：顺序与换行都不丢', () => {
    const text = 'A `@file:"/a.ts"` B `@file:"/b.ts"`\n第二行 `@file:"/c.ts"` 尾巴';
    expect(parseInlineChips(text)).toEqual([
      { type: 'text', text: 'A ' },
      { type: 'chip', path: '/a.ts' },
      { type: 'text', text: ' B ' },
      { type: 'chip', path: '/b.ts' },
      { type: 'text', text: '\n第二行 ' },
      { type: 'chip', path: '/c.ts' },
      { type: 'text', text: ' 尾巴' },
    ]);
  });

  it('整行旧格式（历史消息）也还原成 chip，空行保留在文字里', () => {
    expect(parseInlineChips('看这个\n\n@file:"C:\\a.ts"')).toEqual([
      { type: 'text', text: '看这个\n\n' },
      { type: 'chip', path: 'C:\\a.ts' },
    ]);
  });

  it('序列化 → 解析往返：文本与路径都不变（编辑器回填的正确性依据）', () => {
    const chip = serializeInlineChip('C:\\Program Files\\demo\\main.ts');
    const text = `帮我看下 ${chip} 的架构`;
    expect(parseInlineChips(text)).toEqual([
      { type: 'text', text: '帮我看下 ' },
      { type: 'chip', path: 'C:\\Program Files\\demo\\main.ts' },
      { type: 'text', text: ' 的架构' },
    ]);
    // 反向：把段序列拼回去必须得到原文（受控 value ↔ 编辑器文本的一致性）
    const rebuilt = parseInlineChips(text)
      .map((seg) => (seg.type === 'text' ? seg.text : serializeInlineChip(seg.path)))
      .join('');
    expect(rebuilt).toBe(text);
  });

  it('半截标记（用户手打未完成）不产生 chip，原文原样', () => {
    const text = '把 `@file:"x.ts 放到这里';
    expect(parseInlineChips(text)).toEqual([{ type: 'text', text }]);
  });
});

describe('attachments / fileIconFor', () => {
  it('按扩展名分类（代码 / 文档 / 表格 / 压缩包 / 图片 / 未知）', () => {
    // 用「同类扩展名映射到同一图标组件」断言（不依赖 displayName 之类的实现细节）
    expect(fileIconFor('a.ts').Icon).toBe(fileIconFor('b.rs').Icon);
    expect(fileIconFor('a.md').Icon).toBe(fileIconFor('b.txt').Icon);
    expect(fileIconFor('a.zip').Icon).toBe(fileIconFor('b.tar').Icon);
    expect(fileIconFor('a.csv').Icon).toBe(fileIconFor('b.xlsx').Icon);
    expect(fileIconFor('a.svg').Icon).toBe(fileIconFor('b.bmp').Icon);

    // 不同类别之间必须区分开
    const code = fileIconFor('a.ts').Icon;
    const doc = fileIconFor('a.md').Icon;
    const archive = fileIconFor('a.zip').Icon;
    const sheet = fileIconFor('a.csv').Icon;
    const image = fileIconFor('a.svg').Icon;
    const unknown = fileIconFor('LICENSE').Icon;
    const all = [code, doc, archive, sheet, image, unknown];
    expect(new Set(all).size).toBe(all.length);
  });

  it('pdf 走文档图标但用红色（危险/显眼语义）', () => {
    const pdf = fileIconFor('手册.PDF');
    expect(pdf.Icon).toBe(fileIconFor('a.md').Icon);
    expect(pdf.className).toContain('red');
  });

  it('扩展名大小写不敏感；无扩展名走通用图标', () => {
    expect(fileIconFor('Main.TS').Icon).toBe(fileIconFor('main.ts').Icon);
    expect(fileIconFor('README').Icon).toBe(fileIconFor('noext.weirdext').Icon);
  });
});
