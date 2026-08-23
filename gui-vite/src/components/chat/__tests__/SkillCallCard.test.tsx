import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import SkillCallCard from '@/components/chat/SkillCallCard';

describe('SkillCallCard', () => {
  it('renders skill name and execution time', () => {
    render(
      <SkillCallCard
        info={{ skill_id: 's1', skill_name: 'planning', success: true, execution_time_ms: 320 }}
      />,
    );
    expect(screen.getByText('planning')).toBeInTheDocument();
    expect(screen.getByText('320ms')).toBeInTheDocument();
  });

  it('expands to show skill id and error on failure', () => {
    render(
      <SkillCallCard
        info={{
          skill_id: 's2',
          skill_name: 'web_research',
          success: false,
          execution_time_ms: 50,
          error: '超时',
        }}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /web_research/ }));
    expect(screen.getByText('技能 ID:')).toBeInTheDocument();
    expect(screen.getByText('s2')).toBeInTheDocument();
    expect(screen.getByText('错误:')).toBeInTheDocument();
    expect(screen.getByText('超时')).toBeInTheDocument();
  });
});
