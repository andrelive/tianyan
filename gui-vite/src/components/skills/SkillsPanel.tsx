import { useState } from 'react';
import { useResource } from '@/hooks/use-resource';
import ReactMarkdown from 'react-markdown';
import { useAppStore } from '@/lib/store';
import { apiGet, getSkillDetail, getSkillsStats } from '@/lib/api-client';
import type { Skill, SkillListResponse } from '@/lib/types';
import { Wrench, Loader2, AlertCircle, ChevronRight, Clock } from 'lucide-react';
import ListDetailPanel from '@/components/ui/ListDetailPanel';
import { formatDateTime, formatTimestamp } from '@/lib/utils';

/**
 * 技能面板：只展示方法论技能（custom 类：GEPA 学习技能 + planning）。
 *
 * 内置桥接技能（file/system/network 类）本质是工具的技能化封装，能力已由
 * 工具面板（/tools）与对话中的工具卡片覆盖，不再在此混排。
 * 学习方法论技能为只读展示：完整内容 + 创建/更新时间，无"执行"语义
 * （学习技能无 handler，执行仅返回指南文本——查看更符合其性质）。
 */
export default function SkillsPanel() {
  const skills = useAppStore((s) => s.skills);

  const [selectedSkillId, setSelectedSkillId] = useState<string | null>(null);

  // 只展示方法论技能（custom 类）
  const methodologySkills = skills.filter((s) => s.category === 'custom');
  const selectedSkill: Skill | undefined = methodologySkills.find((s) => s.id === selectedSkillId);

  // Load skills on mount（数据写入 store 技能目录；加载/错误态由 useResource 管理）
  const { loading: loadingSkills, error: skillsError } = useResource(
    async () => {
      const res = await apiGet<SkillListResponse>('/skills');
      useAppStore.getState().setSkills(res.skills);
      return res;
    },
    [],
    { errorFallback: '加载技能失败' },
  );

  // 技能使用统计（哪些技能被调用的多/成功率高）；失败不阻塞面板
  const { data: stats } = useResource(() => getSkillsStats(), [], {
    errorFallback: '加载技能统计失败',
  });

  const skillStats = (id: string) => stats?.skills.find((s) => s.skill_id === id);

  // 详情加载（useResource + enabled：选中即取、切换重取、未选中不取）
  const {
    data: detail,
    loading: loadingDetail,
    error: detailError,
  } = useResource(() => getSkillDetail(selectedSkillId ?? ''), [selectedSkillId], {
    enabled: !!selectedSkillId,
    errorFallback: '加载技能详情失败',
  });

  return (
    <>
      <ListDetailPanel
        header={
          <div className="px-4 py-3 border-b border-[var(--color-border)] flex items-center gap-4 flex-wrap shrink-0">
            {stats && stats.total_calls > 0 ? (
              <>
                <div className="flex items-center gap-2 text-sm">
                  <span className="text-[var(--color-text-secondary)]">技能累计调用</span>
                  <span className="font-semibold text-[var(--color-text-primary)]">
                    {stats.total_calls} 次
                  </span>
                </div>
                <div className="flex items-center gap-1.5 flex-wrap">
                  <span className="text-xs text-[var(--color-text-tertiary)]">
                    使用排行（方法论技能无执行成败语义，仅统计采纳频率）：
                  </span>
                  {stats.skills.slice(0, 8).map((s) => (
                    <span
                      key={s.skill_id}
                      className="inline-block px-1.5 py-0.5 text-[10px] rounded bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-secondary)]"
                      title={`${s.success_calls}/${s.total_calls} 次成功`}
                    >
                      {s.skill_id} {s.total_calls} 次
                    </span>
                  ))}
                </div>
              </>
            ) : (
              <span className="text-xs text-[var(--color-text-tertiary)]">
                暂无技能调用统计——智能体调用技能后，这里会展示调用排行与平均耗时
              </span>
            )}
          </div>
        }
        listHeader={
          <div className="px-4 py-4 border-b border-[var(--color-border)]">
            <h2 className="text-lg font-semibold text-[var(--color-text-primary)] flex items-center gap-2">
              <Wrench size={20} />
              技能
            </h2>
            <p className="text-xs text-[var(--color-text-tertiary)] mt-1">
              自动学习方法论（GEPA 总结），只读查看
            </p>
          </div>
        }

        listLoading={loadingSkills}
        listError={skillsError}
        listAriaLabel="正在加载技能"
        listEmpty={
          methodologySkills.length === 0 ? (
            <>
              <Wrench size={40} className="mb-3 opacity-40" />
              <p className="text-sm">暂无学习方法论技能</p>
              <p className="text-xs mt-1 opacity-70">智能体在会话结束后自动总结高频成功操作</p>
            </>
          ) : null
        }
        list={
          <div className="space-y-1">
            {methodologySkills.map((skill) => (
              <button
                key={skill.id}
                onClick={() => setSelectedSkillId(skill.id)}
                className={`w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors ${
                  selectedSkillId === skill.id
                    ? 'bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300'
                    : 'text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
                }`}
              >
                <div className="flex items-center justify-between">
                  <div className="min-w-0 flex-1">
                    <p className="font-medium truncate">{skill.name}</p>
                    <p className="text-xs text-[var(--color-text-tertiary)] truncate mt-0.5">
                      {skill.description}
                    </p>
                    {(() => {
                      const st = skillStats(skill.id);
                      if (!st || st.total_calls === 0) return null;
                      return (
                        <p className="text-xs mt-1 flex items-center gap-2">
                          <span className="text-[var(--color-text-tertiary)]">
                            调用 {st.total_calls} 次
                          </span>
                          <span className="text-[var(--color-text-tertiary)]/70">
                            均值 {Math.round(st.avg_time_ms)}ms
                          </span>
                          {st.total_calls - st.success_calls > 0 && (
                            <span
                              className="text-red-600 dark:text-red-400"
                              title="handler 执行异常（存储/模型故障），非方法论本身成败"
                            >
                              执行异常 {st.total_calls - st.success_calls} 次
                            </span>
                          )}
                        </p>
                      );
                    })()}
                    {(() => {
                      const rv = stats?.reviews.find((r) => r.skill_id === skill.id);
                      if (!rv) return null;
                      return (
                        <p className="text-xs mt-1 flex items-center gap-2">
                          <span className="text-[var(--color-text-tertiary)]">使用复审</span>
                          <span
                            className={
                              rv.score >= 7
                                ? 'text-emerald-600 dark:text-emerald-400'
                                : rv.score >= 4
                                  ? 'text-amber-600 dark:text-amber-400'
                                  : 'text-red-600 dark:text-red-400'
                            }
                          >
                            {rv.score}/10
                          </span>
                          {rv.verdict === 'negative' && (
                            <span className="text-red-600 dark:text-red-400">效果不佳</span>
                          )}
                        </p>
                      );
                    })()}
                    {skill.updated_at && (
                      <p className="text-xs text-[var(--color-text-tertiary)]/80 flex items-center gap-1 mt-1">
                        <Clock size={11} />
                        更新于 {formatDateTime(skill.updated_at)}
                      </p>
                    )}
                  </div>
                  {selectedSkillId === skill.id && (
                    <ChevronRight size={14} className="shrink-0 ml-2 text-blue-500" />
                  )}
                </div>
              </button>
            ))}
          </div>
        }
        detail={
          !selectedSkill ? (
            <div className="flex-1 flex flex-col items-center justify-center text-[var(--color-text-tertiary)]">
              <Wrench size={48} className="mb-4 opacity-30" />
              <p className="text-sm">选择一个技能查看详情</p>
            </div>
          ) : (
            <>
              {/* Header */}
              <div className="px-6 py-4 border-b border-[var(--color-border)]">
                <h2 className="text-lg font-semibold text-[var(--color-text-primary)]">
                  {selectedSkill.name}
                </h2>
                <p className="text-sm text-[var(--color-text-secondary)] mt-1">
                  {selectedSkill.description}
                </p>
                <div className="flex items-center gap-3 mt-2 flex-wrap">
                  <span className="inline-block px-2 py-0.5 text-xs rounded-full bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-tertiary)]">
                    {selectedSkill.category}
                  </span>
                  {detail?.created_at && (
                    <span className="inline-flex items-center gap-1 text-xs text-[var(--color-text-tertiary)]">
                      <Clock size={11} />
                      创建于 {formatDateTime(detail.created_at)}
                    </span>
                  )}
                  {detail?.updated_at && (
                    <span className="inline-flex items-center gap-1 text-xs text-[var(--color-text-tertiary)]">
                      <Clock size={11} />
                      更新于 {formatDateTime(detail.updated_at)}
                    </span>
                  )}
                </div>
                {(() => {
                  const rv = stats?.reviews.find((r) => r.skill_id === selectedSkill.id);
                  if (!rv) return null;
                  return (
                    <div className="mt-2 p-3 rounded-lg bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-xs">
                      <div className="flex items-center gap-2">
                        <span className="text-[var(--color-text-secondary)]">
                          使用复审（基于会话证据：执行结果 + 用户反馈）
                        </span>
                        <span
                          className={`font-semibold ${rv.score >= 7 ? 'text-emerald-600 dark:text-emerald-400' : rv.score >= 4 ? 'text-amber-600 dark:text-amber-400' : 'text-red-600 dark:text-red-400'}`}
                        >
                          {rv.score}/10
                        </span>
                        {rv.user_feedback === 'negative' && (
                          <span className="text-red-600 dark:text-red-400">用户反馈负面</span>
                        )}
                        <span className="text-[var(--color-text-tertiary)]">
                          复审于 {formatTimestamp(rv.ts)}
                        </span>
                      </div>
                      {rv.reason && (
                        <p className="mt-1.5 text-[var(--color-text-secondary)]">{rv.reason}</p>
                      )}
                    </div>
                  );
                })()}
              </div>

              {/* Content */}
              <div className="flex-1 overflow-y-auto p-6">
                {loadingDetail ? (
                  <div
                    className="flex items-center justify-center py-16"
                    aria-live="polite"
                    aria-label="正在加载技能详情"
                  >
                    <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
                  </div>
                ) : detailError ? (
                  <div
                    role="alert"
                    className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
                  >
                    <AlertCircle size={16} />
                    <span>{detailError}</span>
                  </div>
                ) : detail?.content ? (
                  <div className="prose prose-sm max-w-3xl text-[var(--color-text-primary)] [&_h1]:text-base [&_h2]:text-sm [&_h2]:font-semibold [&_h3]:text-sm [&_h1]:font-bold [&_h2]:mt-4 [&_h1]:mt-2 [&_h3]:mt-3 [&_p]:my-1.5 [&_li]:my-0.5 [&_code]:px-1 [&_code]:py-0.5 [&_code]:rounded [&_code]:text-xs [&_code]:font-mono [&_code]:bg-[var(--color-bg-tertiary)] [&_pre]:p-3 [&_pre]:rounded-lg [&_pre]:bg-[var(--color-bg-tertiary)] [&_pre]:overflow-x-auto [&_pre_code]:bg-transparent [&_table]:text-xs [&_th]:text-left [&_th]:pr-4 [&_th]:py-1 [&_td]:pr-4 [&_td]:py-1 [&_td]:border-t [&_td]:border-[var(--color-border)]">
                    <ReactMarkdown>{detail.content}</ReactMarkdown>
                  </div>
                ) : (
                  <p className="text-sm text-[var(--color-text-tertiary)]">
                    该技能没有可查看的内容。
                  </p>
                )}
              </div>
            </>
          )
        }
      />
    </>
  );
}
