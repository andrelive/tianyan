import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MarkdownContent, SegmentBlocks } from '@/components/chat/MessageSegments';

/** 顶层 mock：@tauri-apps/plugin-opener 仅被 Tauri 分支动态加载。 */
const { openUrl } = vi.hoisted(() => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

/**
 * 回归保护（U4）：文本结构化内容与用户输入的换行渲染。
 *
 * 背景：正文全部走 MarkdownContent（ReactMarkdown）。Markdown 规范把
 * 段落内软换行（单个 \n）渲染为文本节点里的换行，但浏览器 white-space:
 * normal 下 `\n` 与连续空格均被折叠为单个空格——导致模型用文本画线框图
 * （ASCII art）时换行/对齐丢失，用户输入的多行文本同样"挤成一行"。
 *
 * 修复：对段落、以及**不含块级子元素**的列表项应用 whitespace-pre-wrap
 * （保留换行 + 连续空格，且仍允许长行自动换行——区别于 pre）。含块级子元素
 * 的 li 必须排除：hast 会在其首尾插入 \n 文本节点，pre-wrap 会把它们渲染成
 * 整行空行（loose 列表的前导 \n 还会让 marker 与正文分成两行）。
 *
 * 判别力：此测试对修复前的代码必红——容器无 [&_p]/[&_li:not(:has(...))] 的
 * whitespace-pre-wrap 变体类。jsdom 不做 CSS 级联，故以类断言作为
 * "渲染层修复存在"的回归锁；数据链路（文本节点保留 \n）单独断言。
 */
/** li 的 pre-wrap 选择器（须与 MessageSegments.tsx 保持一致）。 */
const LI_PRE_WRAP = '[&_li:not(:has(p,ul,ol,pre,blockquote,table,hr))]:whitespace-pre-wrap';

describe('MarkdownContent 换行渲染（U4）', () => {
  it('段落（assistant 文本）保留软换行：容器对 p/li 应用 whitespace-pre-wrap', () => {
    const text = '┌──────────┐\n│ 前端 → API │\n└──────────┘';
    const { container } = render(<MarkdownContent text={text} isUser={false} />);
    const root = container.firstChild as HTMLElement;

    // 渲染修复点：换行折叠由 pre-wrap 解决（判别力：修复前该类缺失）
    expect(root.className).toContain('[&_p]:whitespace-pre-wrap');
    expect(root.className).toContain(LI_PRE_WRAP);

    // 数据链路保真：文本节点保留换行（换行丢失纯在渲染层，不在数据层）
    const p = container.querySelector('p');
    expect(p?.textContent).toContain('\n');
    expect(p?.textContent?.split('\n')).toHaveLength(3);
  });

  it('用户输入的多行文本走同一渲染路径，同样保留换行', () => {
    const { container } = render(<MarkdownContent text={'第一行\n第二行'} isUser={true} />);
    const root = container.firstChild as HTMLElement;

    expect(root.className).toContain('[&_p]:whitespace-pre-wrap');
    expect(container.querySelector('p')?.textContent?.split('\n')).toHaveLength(2);
  });

  it('列表项场景覆盖（li 变体）：多行列表不折叠', () => {
    const { container } = render(<MarkdownContent text={'- 第一项\n- 第二项'} isUser={false} />);
    const root = container.firstChild as HTMLElement;

    expect(root.className).toContain(LI_PRE_WRAP);
    expect(container.querySelectorAll('li')).toHaveLength(2);
    // tight 列表项：直接内联内容（无块级子元素、无 hast 插入的 \n）→ pre-wrap 生效
    const tightLi = container.querySelector('li');
    expect(tightLi?.childNodes).toHaveLength(1);
    expect(tightLi?.firstChild?.nodeType).toBe(3);
  });

  it('含块级子元素的列表项排除 pre-wrap（否则 hast 的 \n 渲染成整行空行）', () => {
    // 回归（用户报告：二级无序列表 / 有序列表项之间多出空行，marker 与正文被挤成两行）：
    // hast 会给含块级子元素的 li 在首尾插入 \n 文本节点——loose 列表在 <p> 前后、
    // 嵌套列表在「文本 → <ul>」之间。li 上的 pre-wrap 会把这些 \n 渲染成整行空行
    // （浏览器实测：松散列表每项 +47px、嵌套列表块高翻倍）。修复 = pre-wrap 只落在
    // 不含块级子元素的 li 上；含块级子元素时，段落软换行由 [&_p] 继续保证。
    const loose = render(<MarkdownContent text={'1. 第一项\n\n2. 第二项'} isUser={false} />);
    const looseRoot = loose.container.firstChild as HTMLElement;
    expect(looseRoot.className).toContain(LI_PRE_WRAP);
    // 防回退：不得退回裸 li 选择器（拼接写法规避 Tailwind 内容扫描把该字面量
    // 当类名提取——否则构建产物里会多出一条永不命中的死规则）
    expect(looseRoot.className).not.toContain('[&_li]' + ':whitespace-pre-wrap');

    // 触发条件确凿：loose 的 li 首个子节点就是 hast 插入的前导 \n（会被渲染成空行）
    const looseLi = loose.container.querySelector('li');
    expect(looseLi?.firstChild?.nodeValue).toBe('\n');
    expect(loose.container.querySelector('li p')).not.toBeNull();

    // 嵌套列表同样是触发条件：li 内「文本 → \n → <ul>」
    const nested = render(<MarkdownContent text={'- 父项：\n  - 子项'} isUser={false} />);
    expect(nested.container.querySelector('li ul')).not.toBeNull();
    const nestedLi = nested.container.querySelector('li');
    expect(nestedLi?.childNodes.length).toBeGreaterThan(1);
  });
});

/**
 * 回归保护（U6）：**无语言标记**的围栏代码块换行渲染。
 *
 * 背景：`pre` 组件被透明化（`<>{children}</>`）——有语言的代码块走
 * SyntaxHighlighter（自带容器），但无语言的围栏块仅剩 inline `<code>`
 * （无 white-space: pre），多行内容被折叠成一行（用户报告：目录树挤成一行）。
 *
 * 修复：`code` 组件按 ReactMarkdown 约定（无语言块级 children 以 \n 结尾）
 * 区分块级/内联——块级渲染自带 pre 语义的容器。
 *
 * 判别力：修复前无 `<pre>` 元素（被透明化），第一条断言必红。
 */
describe('MarkdownContent 围栏代码块换行（U6）', () => {
  it('无语言围栏块保留块级容器（pre 语义 + whitespace-pre）', () => {
    const text = '```\n<新仓>/\n├── EmergencyBackend/\n└── opencode.json\n```';
    const { container } = render(<MarkdownContent text={text} isUser={false} />);

    const pre = container.querySelector('pre');
    expect(pre).not.toBeNull(); // 修复前为 null（pre 被透明化）
    expect(pre!.className).toContain('whitespace-pre'); // CSS 保留换行
    expect(pre!.textContent).toContain('EmergencyBackend');
    expect(pre!.textContent?.split('\n').length).toBeGreaterThanOrEqual(3);
  });

  it('行内代码不受影响（不产生块级容器）', () => {
    const { container } = render(<MarkdownContent text={'这是 `inline` 代码'} isUser={false} />);
    expect(container.querySelector('pre')).toBeNull();
    expect(container.querySelector('code')?.textContent).toBe('inline');
  });

  it('有语言标记的代码块仍走高亮器容器（不回归）', () => {
    const { container } = render(<MarkdownContent text={'```bash\nls -la\n```'} isUser={false} />);
    expect(container.textContent).toContain('ls -la');
    // 高亮器用 div 容器（PreTag="div"）——不重复包 <pre>
    expect(container.querySelector('pre')).toBeNull();
  });
});

/**
 * 回归保护（A+B）：消息内链接的渲染修复与点击兜底。
 *
 * A（渲染）：GFM autolink 会把紧跟 URL 的 `**` 一并吞掉（`**http://x**（91`），
 * 导致星号字面泄露且 href 被污染（点坏地址 / 触发 WebView 异常导航后白屏）。
 * remark-autolink-fix 在 AST 层截断并还原配对。
 *
 * B（点击）：桌面壳内点击 http(s) 链接一律交给系统默认浏览器（preventDefault
 * 阻止 WebView 自行导航——Tauri 对「URL 解析失败的导航」静默放行，导航失败即
 * 白屏且无回退入口）；非 Tauri 环境不接管，保持浏览器默认行为（E2E 零影响）。
 *
 * 判别力：A 断言对未接插件的代码必红（href 会是 `http://localhost:5180**（91`）；
 * B 的 Tauri 分支对未加 onClick 的代码必红（openUrl 不会被调用）。
 */
describe('消息内链接：渲染修复 + 点击兜底（A+B）', () => {
  beforeEach(() => {
    openUrl.mockClear();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  /** 派发可取消的点击事件（便于断言 defaultPrevented）。 */
  function clickLink(a: HTMLAnchorElement): MouseEvent {
    const e = new MouseEvent('click', { bubbles: true, cancelable: true });
    a.dispatchEvent(e);
    return e;
  }

  it('A：被污染的链接渲染为干净 href，正文无 ** 残留', () => {
    const { container } = render(
      <MarkdownContent text={'- **http://localhost:5180**（91 视图'} isUser={false} />,
    );
    const a = container.querySelector('a');
    expect(a).not.toBeNull();
    expect(a!.getAttribute('href')).toBe('http://localhost:5180');
    expect(container.textContent).not.toContain('**');
    expect(container.querySelector('strong a')).not.toBeNull();
  });

  it('B：非 Tauri 环境（浏览器 / E2E）不接管，默认行为保留', () => {
    const { container } = render(
      <MarkdownContent text={'[示例](https://example.com)'} isUser={false} />,
    );
    const e = clickLink(container.querySelector('a')!);
    expect(e.defaultPrevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
  });

  it('B：Tauri 环境点击 http(s) 链接 → preventDefault + 交给系统浏览器', async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const { container } = render(
      <MarkdownContent text={'[示例](https://example.com)'} isUser={false} />,
    );
    const e = clickLink(container.querySelector('a')!);
    expect(e.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(openUrl).toHaveBeenCalledWith('https://example.com'));
  });

  it('B：非 http 链接（页内锚点）不接管', () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const { container } = render(<MarkdownContent text={'[锚点](#section)'} isUser={false} />);
    const e = clickLink(container.querySelector('a')!);
    expect(e.defaultPrevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
  });
});

/**
 * 文件引用通道（批次 1）的回显：正文里的 `@file:"..."` 行还原成只读 chip。
 *
 * 判别力：未接 parseAttachmentLines 的代码会把路径行当正文渲染（找不到
 * chip、且正文里能读到 `@file:` 字面量）。
 */
describe('文件引用回显', () => {
  it('正文中的引用行渲染为只读 chip（无删除按钮），其余正文仍走 Markdown', () => {
    render(
      <SegmentBlocks
        segments={[
          { type: 'text', text: '看这个\n\n@file:"C:\\a b\\main.ts"\n@file:"/x/readme.md"' },
        ]}
        isUser={true}
      />,
    );

    const chips = screen.getAllByTestId('file-chip');
    expect(chips).toHaveLength(2);
    expect(chips[0]).toHaveTextContent('main.ts');
    expect(chips[0]).toHaveAttribute('title', 'C:\\a b\\main.ts');
    expect(chips[1]).toHaveTextContent('readme.md');
    // 只读：消息区不提供删除（发出去的消息不该能改）
    expect(screen.queryByRole('button', { name: /移除文件/ })).not.toBeInTheDocument();

    // 正文不再泄露路径行，Markdown 结构保留
    const body = screen.getByText(/看这个/);
    expect(body.textContent).not.toContain('@file:');
  });

  it('普通文本（无引用行）渲染路径不变', () => {
    const { container } = render(
      <SegmentBlocks segments={[{ type: 'text', text: '第一行\n第二行' }]} isUser={false} />,
    );
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    expect(container.querySelector('p')?.textContent).toContain('第一行');
  });
});

/**
 * 行内格式（新消息）：`` `@file:"<路径>"` `` 是 chip 的序列化形态，消息区必须
 * **原位**渲染成 chip（用户当初粘在句子里，就显示在句子里）。
 *
 * 判别力：没接 code 渲染器的实现会把反引号内容渲染成 `<code>@file:"…"</code>`
 * ——用户看到一条裸路径，并且找不到 chip。
 */
describe('文件引用回显（行内格式，原位）', () => {
  it('行内 code 渲染为原位 chip：前后文字仍在同一段落里', () => {
    const { container } = render(
      <SegmentBlocks
        segments={[
          { type: 'text', text: '帮我看下 `@file:"C:\\a b\\张磊--系统架构.docx"` 的架构' },
        ]}
        isUser={true}
      />,
    );

    const chip = screen.getByTestId('file-chip');
    expect(chip).toHaveTextContent('张磊--系统架构.docx');
    expect(chip).toHaveAttribute('title', 'C:\\a b\\张磊--系统架构.docx');
    // 原位：chip 在段落内部（不是被提到正文之前的独立 chip 行）
    const paragraph = container.querySelector('p');
    expect(paragraph?.contains(chip)).toBe(true);
    // 前后文字都还在这个段落里，且不再有裸路径
    expect(paragraph?.textContent).toContain('帮我看下');
    expect(paragraph?.textContent).toContain('的架构');
    expect(container.querySelector('code')).toBeNull();
    // 只读：消息区不给删除按钮
    expect(screen.queryByRole('button', { name: /移除文件/ })).not.toBeInTheDocument();
  });

  it('一行多个 chip + 跨行：各自原位，顺序不乱', () => {
    const { container } = render(
      <SegmentBlocks
        segments={[
          {
            type: 'text',
            text: '先看 `@file:"/a.ts"` 再看 `@file:"/b.md"`\n\n完毕',
          },
        ]}
        isUser={false}
      />,
    );

    const chips = screen.getAllByTestId('file-chip');
    expect(chips.map((c) => c.getAttribute('title'))).toEqual(['/a.ts', '/b.md']);
    expect(chips[0].compareDocumentPosition(chips[1])).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(container.textContent).toContain('先看');
    expect(container.textContent).toContain('完毕');
  });

  it('围栏代码块里的 @file: 字样不被误判成 chip（含无语言标记的块）', () => {
    // 直接测 MarkdownContent：SegmentBlocks 还会跑旧格式（整行）提取，
    // 这里只想锁定「code 渲染器不把代码块当 chip」这一条。
    const fenced = '```\n@file:"C:\\a.ts"\n```';
    const { container } = render(<MarkdownContent text={fenced} isUser={false} />);
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    // 仍是代码块（pre + code），内容原样
    expect(container.querySelector('pre')?.textContent).toContain('@file:"C:\\a.ts"');
  });

  it('带语言标记的围栏块：语法高亮优先，不当 chip', () => {
    const { container } = render(
      <MarkdownContent text={'```text\n@file:"C:\\a.ts"\n```'} isUser={false} />,
    );
    expect(screen.queryByTestId('file-chip')).not.toBeInTheDocument();
    expect(container.textContent).toContain('@file:');
  });
});
