/**
 * remark 插件：修复 GFM autolink literal 的"定界符吞噬 / 粘连污染"。
 *
 * 背景（实测 AST）：`**http://localhost:5180**（91 视图` 被 autolink 吞成
 *
 *   text("**") + link(url="http://localhost:5180**（91") + text(" 视图")
 *
 * autolink 把 URL 一路吞到下一个空白；URL 末尾是数字/全角标点时"尾标点修剪"
 * 不生效，闭合 `**` 被吞进 URL → 星号字面泄露 + href 被污染（点击跳错地址或
 * 触发 WebView 异常导航后白屏）。
 *
 * 修复（AST 后处理，须排在 remark-gfm 之后）：
 *  1. link.url 在首个 `**` 处截断；
 *  2. 若前一个兄弟 text 以 `**` 结尾，则"该 `**` + 修好的 link"重组为 strong
 *     （还原粗体配对）；否则仅截断（裸 URL 场景）；
 *  3. 被剥出的尾巴还原为文本（`**` 视为已消费的定界符，不显示）；
 *  4. 保守附加规则：URL 的 authority 之后直接粘全角标点/中文时，从粘连处截断
 *     （只处理"host(:port) 后直接粘中文/全角"的形态，不动带 path 的 URL）。
 *
 * 依赖：mdast 树中 `link` / `text` / `strong` 的最小子集结构（不引入
 * @types/mdast，保持与 remark 的宽松耦合）。
 */

/** mdast 节点最小结构。 */
interface MdNode {
  type: string;
  value?: string;
  url?: string;
  children?: MdNode[];
}

/** CJK 与全角标点（用于识别"URL 主体之后的粘连尾巴"）。 */
const CJK_OR_FULLWIDTH = /[\u2E80-\u9FFF\u3000-\u303F\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFFEF]/;

/**
 * 定位 URL 中"粘连尾巴"的起点；返回 -1 表示 URL 干净。
 *
 * ① 首个 `**`（markdown 强调定界符被 autolink 吞入）；
 * ② 保守规则：`scheme://authority` 段内出现 CJK / 全角标点
 *    （例：`http://localhost:5180（91 视图` —— authority 后直接粘中文）。
 */
export function findAutolinkTailStart(url: string): number {
  const bold = url.indexOf('**');
  if (bold >= 0) return bold;

  const schemeEnd = url.indexOf('://');
  if (schemeEnd < 0) return -1;
  const authorityStart = schemeEnd + 3;
  const slash = url.indexOf('/', authorityStart);
  const authorityEnd = slash < 0 ? url.length : slash;
  for (let i = authorityStart; i < authorityEnd; i += 1) {
    if (CJK_OR_FULLWIDTH.test(url[i])) return i;
  }
  return -1;
}

/** 修复一个 children 数组，返回重建后的数组（自底向上，避免原地 splice 的索引漂移）。 */
function fixChildren(children: MdNode[]): MdNode[] {
  const out: MdNode[] = [];

  for (const node of children) {
    if (Array.isArray(node.children)) {
      node.children = fixChildren(node.children);
    }

    if (node.type === 'link' && typeof node.url === 'string') {
      const start = findAutolinkTailStart(node.url);
      if (start > 0) {
        const head = node.url.slice(0, start);
        const tail = node.url.slice(start);
        const tailText = tail.startsWith('**') ? tail.slice(2) : tail;

        // 截断链接：href 与显示文本都收敛到干净 URL
        node.url = head;
        node.children = [{ type: 'text', value: head }];

        const prev = out.length > 0 ? out[out.length - 1] : undefined;
        if (
          prev &&
          prev.type === 'text' &&
          typeof prev.value === 'string' &&
          prev.value.endsWith('**')
        ) {
          const rest = prev.value.slice(0, -2);
          if (rest.length > 0) {
            prev.value = rest;
          } else {
            out.pop();
          }
          out.push({ type: 'strong', children: [node] });
        } else {
          out.push(node);
        }

        if (tailText.length > 0) {
          out.push({ type: 'text', value: tailText });
        }
        continue;
      }
    }

    out.push(node);
  }

  return mergeAdjacentText(out);
}

/** 合并相邻 text 节点（修复过程会产生碎片；归一化减少节点数）。 */
function mergeAdjacentText(nodes: MdNode[]): MdNode[] {
  const out: MdNode[] = [];
  for (const node of nodes) {
    const prev = out.length > 0 ? out[out.length - 1] : undefined;
    if (
      prev &&
      prev.type === 'text' &&
      node.type === 'text' &&
      typeof prev.value === 'string' &&
      typeof node.value === 'string'
    ) {
      prev.value += node.value;
      continue;
    }
    out.push(node);
  }
  return out;
}

/**
 * remark 插件：在 `run` 阶段（必须排在 remark-gfm 之后）修复被污染的链接。
 */
export default function remarkAutolinkFix() {
  return (tree: MdNode) => {
    if (Array.isArray(tree.children)) {
      tree.children = fixChildren(tree.children);
    }
  };
}
