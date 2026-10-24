import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import DelegateActivity, { TaskLiveStep } from '../DelegateActivity';
import { useAppStore } from '@/lib/store';
import type { ChatMessage, MessageSegment, ToolCallWithResult } from '@/lib/types';

/**
 * 子智能体活动流（activity feed）。
 *
 * 这些用例同时是**设计约束的守门人**：活动流是「已发生时间线的无损投影」，
 * 不得反过来要求子智能体按某种流程工作。所以退化场景（零工具调用 / 只有思考 /
 * 连续失败 / 空记录）必须全部成立，且**不得出现「N/M」式进度分数**
 * （总数意味着既定计划 = 隐性协议约束）。
 */

type ToolSpec = {
  id: string;
  name: string;
  presentation: string;
  arguments: string;
  success?: boolean;
  duration_ms?: number;
  error?: string | null;
  result?: string | null;
};

/** 造一条 assistant 消息（段顺序 = 真实到达顺序：思考 → 工具 → 正文）。 */
function assistant(parts: { thinking?: string; tools?: ToolSpec[]; text?: string }): ChatMessage {
  const segments: MessageSegment[] = [];
  if (parts.thinking) segments.push({ type: 'thinking', text: parts.thinking });
  for (const t of parts.tools ?? []) {
    segments.push({
      type: 'tool',
      tool_call: { id: t.id, name: t.name, presentation: t.presentation, arguments: t.arguments },
    });
  }
  if (parts.text) segments.push({ type: 'text', text: parts.text });

  const tool_calls: ToolCallWithResult[] = (parts.tools ?? []).map((t) => ({
    id: t.id,
    name: t.name,
    presentation: t.presentation,
    arguments: t.arguments,
    success: t.success,
    duration_ms: t.duration_ms,
    error: t.error,
    result: t.result,
  }));

  return {
    role: 'assistant',
    segments,
    ...(tool_calls.length > 0 ? { tool_calls } : {}),
  };
}

function mount(messages: ChatMessage[], status = 'completed') {
  useAppStore.setState({ sessionMessages: { bt_1: messages } });
  return render(<DelegateActivity taskId="bt_1" status={status} />);
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
});

describe('DelegateActivity', () => {
  it('renders one compact line per tool call (label + target + duration), expandable to raw content', () => {
    mount([
      assistant({
        tools: [
          {
            id: 'c1',
            name: 'read_file',
            presentation: 'read',
            arguments: '{"path":"src/api/mod.rs"}',
            success: true,
            duration_ms: 12,
            result: 'fn main() {}',
          },
        ],
      }),
    ]);

    expect(screen.getByText('读取文件')).toBeInTheDocument();
    expect(screen.getByText('src/api/mod.rs')).toBeInTheDocument();
    expect(screen.getByText('12ms')).toBeInTheDocument();

    // 无损：结果默认收起，点开后能看到原文（不是压缩掉信息换高度）
    expect(screen.queryByText('fn main() {}')).not.toBeInTheDocument();
    fireEvent.click(screen.getByText('读取文件').closest('button') as HTMLElement);
    expect(screen.getByText('fn main() {}')).toBeInTheDocument();
  });

  it('extracts the target file from an apply_patch envelope', () => {
    mount([
      assistant({
        tools: [
          {
            id: 'p1',
            name: 'apply_patch',
            presentation: 'diff',
            arguments:
              '*** Begin Patch\n*** Update File: src/a.ts\n@@\n-old\n+new\n*** End Patch\n',
            success: true,
            duration_ms: 7,
          },
        ],
      }),
    ]);
    expect(screen.getByText('编辑文件')).toBeInTheDocument();
    expect(screen.getByText('src/a.ts')).toBeInTheDocument();
  });

  // ── 退化场景：任何工作方式都必须能显示（这是「不约束子智能体」的验收条件）──

  it('零工具调用：只有正文时只出一行「输出」，不虚构步骤', () => {
    mount([assistant({ text: '直接给结论：改好了。' })]);

    expect(screen.getByText('输出')).toBeInTheDocument();
    expect(screen.getByText('直接给结论：改好了。')).toBeInTheDocument();
    // 不得凭空出现步骤/阶段结构
    expect(screen.queryByText(/步骤|阶段|第 ?1 ?步/)).not.toBeInTheDocument();
  });

  it('只有思考、零输出零工具时也能显示', () => {
    mount([assistant({ thinking: '嗯，先想想要不要动手' })]);
    expect(screen.getByText('思考')).toBeInTheDocument();
    expect(screen.getByText('嗯，先想想要不要动手')).toBeInTheDocument();
  });

  it('连续失败的工具调用逐个保留（不折叠、不吞掉），展开可见失败原因', () => {
    mount([
      assistant({
        tools: [
          {
            id: 'e1',
            name: 'execute_command',
            presentation: 'terminal',
            arguments: '{"command":"cargo check"}',
            success: false,
            duration_ms: 30,
            error: 'exit code 1',
          },
          {
            id: 'e2',
            name: 'execute_command',
            presentation: 'terminal',
            arguments: '{"command":"cargo test"}',
            success: false,
            duration_ms: 40,
            error: 'timeout',
          },
        ],
      }),
    ]);

    // 两行都在（同标签，用命令目标区分）
    expect(screen.getByText('cargo check')).toBeInTheDocument();
    expect(screen.getByText('cargo test')).toBeInTheDocument();

    fireEvent.click(screen.getByText('cargo check').closest('button') as HTMLElement);
    expect(screen.getByText('exit code 1')).toBeInTheDocument();
  });

  it('空过程记录：运行中显示「子智能体工作中…」，已结束显示「无过程记录」', () => {
    useAppStore.setState({ sessionMessages: { bt_1: [] } });
    const running = render(<DelegateActivity taskId="bt_1" status="running" />);
    expect(screen.getByText('子智能体工作中…')).toBeInTheDocument();
    running.unmount();

    const finished = render(<DelegateActivity taskId="bt_1" status="completed" />);
    expect(screen.getByText('无过程记录')).toBeInTheDocument();
    finished.unmount();
  });

  it('永不渲染「N/M」式进度分数（总数暗示既定计划 = 隐性约束）', () => {
    const { container } = mount([
      assistant({
        tools: [1, 2, 3].map((n) => ({
          id: `c${n}`,
          name: 'glob',
          presentation: 'search',
          arguments: `{"pattern":"*.rs"}`,
          success: true,
          duration_ms: n,
        })),
      }),
    ]);
    expect(container.textContent ?? '').not.toMatch(/\d+\s*\/\s*\d+/);
  });

  it('保持真实到达顺序（思考 → 工具 → 正文），不重排、不归类', () => {
    mount([
      assistant({
        thinking: '先读一下',
        tools: [
          {
            id: 'c1',
            name: 'read_file',
            presentation: 'read',
            arguments: '{"path":"a.ts"}',
            success: true,
            duration_ms: 3,
          },
        ],
        text: '读完了，可以改',
      }),
    ]);
    const text = document.body.textContent ?? '';
    const iThink = text.indexOf('思考');
    const iTool = text.indexOf('读取文件');
    const iOut = text.indexOf('输出');
    expect(iThink).toBeGreaterThanOrEqual(0);
    expect(iTool).toBeGreaterThan(iThink);
    expect(iOut).toBeGreaterThan(iTool);
  });
});

describe('TaskLiveStep', () => {
  it('shows the latest tool step on the task row', () => {
    useAppStore.setState({
      sessionMessages: {
        bt_1: [
          assistant({
            tools: [
              {
                id: 'p1',
                name: 'apply_patch',
                presentation: 'diff',
                arguments: '*** Update File: src/a.ts\n',
                duration_ms: 7,
              },
            ],
          }),
        ],
      },
    });
    render(<TaskLiveStep taskId="bt_1" />);
    expect(screen.getByText('编辑文件 src/a.ts')).toBeInTheDocument();
  });

  it('renders nothing when the session has no messages', () => {
    useAppStore.setState({ sessionMessages: {} });
    const { container } = render(<TaskLiveStep taskId="bt_missing" />);
    expect(container).toBeEmptyDOMElement();
  });
});
