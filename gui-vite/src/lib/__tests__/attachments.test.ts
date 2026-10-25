import { describe, it, expect } from 'vitest';
import {
  appendAttachments,
  basenameOf,
  fileIconFor,
  formatAttachmentLines,
  parseAttachmentLines,
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

describe('attachments / formatAttachmentLines', () => {
  it('一行一个文件，路径用双引号包裹（含空格路径可解析）', () => {
    expect(formatAttachmentLines([WIN_REF, POSIX_REF])).toBe(
      '@file:"C:\\Program Files\\demo\\main.ts"\n@file:"/home/u/repo/src/lib.rs"',
    );
  });

  it('空列表 → 空串', () => {
    expect(formatAttachmentLines([])).toBe('');
  });
});

describe('attachments / parseAttachmentLines', () => {
  it('提取引用行并保留正文；format/parse 往返一致', () => {
    const text = formatAttachmentLines([WIN_REF, POSIX_REF]);
    const parsed = parseAttachmentLines(text);
    expect(parsed.body).toBe('');
    expect(parsed.refs).toEqual([WIN_REF, POSIX_REF]);
  });

  it('含空格路径 + 正文往返一致', () => {
    const content = appendAttachments('看这个文件', [WIN_REF]);
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

describe('attachments / appendAttachments', () => {
  it('无引用时原样返回（不做 trim）', () => {
    expect(appendAttachments('原样 文本\n', [])).toBe('原样 文本\n');
  });

  it('去掉正文尾部空白后以空行分隔引用行', () => {
    expect(appendAttachments('正文   \n\n', [POSIX_REF])).toBe(
      '正文\n\n@file:"/home/u/repo/src/lib.rs"',
    );
  });

  it('正文为空（纯附件）时不加前导空行', () => {
    expect(appendAttachments('   ', [POSIX_REF])).toBe('@file:"/home/u/repo/src/lib.rs"');
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
