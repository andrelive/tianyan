/** SkillsPanel 测试（G3：此前唯一无直接测试的页面）。
 *
 * 覆盖：方法论技能过滤（只展示 custom 类）、选中加载详情、
 * 统计概览渲染、错误态、空态。MSW 已有 /skills 端点；
 * /skills/stats 与 /skills/:id 在测试内 server.use 注入。
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { server } from '@/test/mocks/server';
import { http, HttpResponse } from 'msw';
import { useAppStore } from '@/lib/store';
import type { Skill, SkillsStatsResponse } from '@/lib/types';
import SkillsPanel from '../SkillsPanel';

const API_BASE = '/api/v1';

/** custom 类方法论技能（面板只展示这类）。 */
const METHODOLOGY_SKILLS: Skill[] = [
  {
    id: 'planning',
    name: 'planning',
    description: '任务规划方法论',
    category: 'custom',
    version: '1.0.0',
    enabled: true,
    updated_at: '2026-08-01T00:00:00Z',
  },
  {
    id: 'gepa-1',
    name: 'gepa-1',
    description: 'GEPA 学习技能',
    category: 'custom',
    version: '1.0.0',
    enabled: true,
  },
];

/** 内置桥接技能（不应出现在面板）。 */
const BRIDGE_SKILLS: Skill[] = [
  {
    id: 'file-read',
    name: 'file-read',
    description: '文件读取桥接',
    category: '文件操作',
    version: '1.0.0',
    enabled: true,
  },
];

const STATS: SkillsStatsResponse = {
  skills: [
    {
      skill_id: 'planning',
      total_calls: 5,
      success_calls: 4,
      success_rate: 0.8,
      avg_time_ms: 1200,
      last_called_at: '2026-08-01T00:00:00Z',
    },
  ],
  reviews: [],
  total_calls: 5,
  total_success: 4,
  success_rate: 0.8,
};

function renderPanel() {
  return render(<SkillsPanel />);
}

beforeEach(() => {
  useAppStore.setState(useAppStore.getInitialState());
  server.use(
    http.get(`${API_BASE}/skills`, () =>
      HttpResponse.json({ skills: [...METHODOLOGY_SKILLS, ...BRIDGE_SKILLS] }),
    ),
    http.get(`${API_BASE}/skills/stats`, () => HttpResponse.json(STATS)),
    http.get(`${API_BASE}/skills/:id`, ({ params }) => {
      const skill = METHODOLOGY_SKILLS.find((s) => s.id === params.id);
      if (!skill) return new HttpResponse(null, { status: 404 });
      return HttpResponse.json({ ...skill, content: `# ${skill.name}\n完整内容` });
    }),
  );
});

describe('SkillsPanel', () => {
  it('renders only methodology skills (custom category), filtering bridge skills', async () => {
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText('planning')).toBeInTheDocument();
    });
    expect(screen.getByText('gepa-1')).toBeInTheDocument();
    expect(screen.queryByText('file-read')).not.toBeInTheDocument();
  });

  it('shows the empty state when no methodology skills exist', async () => {
    server.use(http.get(`${API_BASE}/skills`, () => HttpResponse.json({ skills: BRIDGE_SKILLS })));
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText('暂无学习方法论技能')).toBeInTheDocument();
    });
  });

  it('shows an error banner when the skills endpoint fails', async () => {
    server.use(
      http.get(`${API_BASE}/skills`, () => HttpResponse.json({ message: 'boom' }, { status: 500 })),
    );
    renderPanel();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(/boom|加载技能失败/);
    });
  });

  it('loads and renders skill detail when a skill is selected', async () => {
    const user = userEvent.setup();
    renderPanel();
    await user.click(await screen.findByText('planning'));
    await waitFor(() => {
      expect(screen.getByText('完整内容')).toBeInTheDocument();
    });
    // 描述同时出现在列表行与详情头 → 至少两处
    expect(screen.getAllByText('任务规划方法论').length).toBeGreaterThanOrEqual(2);
  });

  it('renders usage stats for skills with calls', async () => {
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText('技能累计调用')).toBeInTheDocument();
    });
    expect(screen.getByText('5 次')).toBeInTheDocument();
    expect(screen.getByText(/planning 5 次/)).toBeInTheDocument();
  });

  it('shows the no-stats hint when stats are empty', async () => {
    server.use(
      http.get(`${API_BASE}/skills/stats`, () =>
        HttpResponse.json({
          skills: [],
          reviews: [],
          total_calls: 0,
          total_success: 0,
          success_rate: 0,
        }),
      ),
    );
    renderPanel();
    await waitFor(() => {
      expect(screen.getByText(/暂无技能调用统计/)).toBeInTheDocument();
    });
  });
});
