import { useState, useEffect, useCallback } from 'react';
import { useAppStore } from '@/lib/store';
import { apiGet, apiPost } from '@/lib/api-client';
import { groupByCategory } from '@/lib/utils';
import type {
  SkillParameter,
  SkillExecutionStatus,
  SkillListResponse,
  SkillExecuteResponse,
} from '@/lib/types';
import { Wrench, Play, Loader2, CheckCircle2, AlertCircle, ChevronRight } from 'lucide-react';

type ParamValues = Record<string, string | number | boolean>;

function ParameterInput({
  param,
  value,
  onChange,
}: {
  param: SkillParameter;
  value: string | number | boolean;
  onChange: (name: string, val: string | number | boolean) => void;
}) {
  const baseClass =
    'w-full px-3 py-2 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-sm text-[var(--color-text-primary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500 transition-colors';

  if (param.type === 'boolean') {
    return (
      <label className="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={value === true}
          onChange={(e) => onChange(param.name, e.target.checked)}
          className="rounded border-[var(--color-border)] text-blue-600 focus:ring-blue-500"
        />
        <span className="text-sm text-[var(--color-text-primary)]">
          {param.description || param.name}
        </span>
      </label>
    );
  }

  if (param.type === 'number') {
    return (
      <input
        type="number"
        value={value as number}
        onChange={(e) => onChange(param.name, e.target.value === '' ? '' : Number(e.target.value))}
        placeholder={param.description || param.name}
        className={baseClass}
      />
    );
  }

  return (
    <input
      type="text"
      value={value as string}
      onChange={(e) => onChange(param.name, e.target.value)}
      placeholder={param.description || param.name}
      className={baseClass}
    />
  );
}

export default function SkillsPanel() {
  const skills = useAppStore((s) => s.skills);
  const setSkills = useAppStore((s) => s.setSkills);

  // Skills list
  const [loadingSkills, setLoadingSkills] = useState(true);
  const [skillsError, setSkillsError] = useState<string | null>(null);

  // Selection
  const [selectedSkillId, setSelectedSkillId] = useState<string | null>(null);
  const selectedSkill = skills.find((s) => s.id === selectedSkillId) ?? null;

  // Parameter values
  const [paramValues, setParamValues] = useState<ParamValues>({});

  // Execution
  const [executing, setExecuting] = useState(false);
  const [executionStatus, setExecutionStatus] = useState<SkillExecutionStatus | null>(null);

  // Load skills on mount
  useEffect(() => {
    let cancelled = false;

    async function load() {
      setLoadingSkills(true);
      setSkillsError(null);
      try {
        const res = await apiGet<SkillListResponse>('/skills');
        if (!cancelled) {
          setSkills(res.skills);
        }
      } catch (err) {
        if (!cancelled) {
          setSkillsError(err instanceof Error ? err.message : '加载技能失败');
        }
      } finally {
        if (!cancelled) {
          setLoadingSkills(false);
        }
      }
    }

    load();
    return () => {
      cancelled = true;
    };
  }, [setSkills]);

  // Initialize param values when skill changes
  useEffect(() => {
    if (selectedSkill) {
      const initial: ParamValues = {};
      for (const p of selectedSkill.parameters ?? []) {
        if (p.default_value !== undefined && p.default_value !== null) {
          initial[p.name] = p.default_value as string | number | boolean;
        } else if (p.type === 'boolean') {
          initial[p.name] = false;
        } else if (p.type === 'number') {
          initial[p.name] = 0;
        } else {
          initial[p.name] = '';
        }
      }
      setParamValues(initial);
      setExecutionStatus(null);
    }
  }, [selectedSkillId, skills]);

  const handleParamChange = useCallback((name: string, val: string | number | boolean) => {
    setParamValues((prev) => ({ ...prev, [name]: val }));
  }, []);

  const handleExecute = async () => {
    if (!selectedSkill) return;

    setExecuting(true);
    setExecutionStatus(null);

    try {
      // Convert param values to strings for the API
      const params: Record<string, string> = {};
      for (const [key, val] of Object.entries(paramValues)) {
        params[key] = String(val);
      }

      const res = await apiPost<SkillExecuteResponse>(`/skills/${selectedSkill.id}/execute`, {
        parameters: params,
      });

      // 同步执行器：execute 响应即最终结果，直接展示（无异步轮询）
      if (res.success) {
        setExecutionStatus({
          status: 'completed',
          job_id: res.job_id,
          skill_id: res.skill_id,
          result: res.result,
        });
      } else {
        setExecutionStatus({
          status: 'failed',
          error: res.error || res.message || '执行技能失败',
        });
      }
      setExecuting(false);
    } catch (err) {
      setExecutionStatus({
        status: 'failed',
        error: err instanceof Error ? err.message : '执行技能失败',
      });
      setExecuting(false);
    }
  };

  const grouped = groupByCategory(skills);

  return (
    <div className="flex h-full">
      {/* Left panel: skill list */}
      <div className="w-[40%] min-w-[260px] max-w-[360px] flex flex-col border-r border-[var(--color-border)]">
        <div className="px-4 py-4 border-b border-[var(--color-border)]">
          <h2 className="text-lg font-semibold text-[var(--color-text-primary)] flex items-center gap-2">
            <Wrench size={20} />
            技能
          </h2>
        </div>

        <div className="flex-1 overflow-y-auto p-3">
          {loadingSkills ? (
            <div
              className="flex items-center justify-center py-16"
              aria-live="polite"
              aria-label="正在加载技能"
            >
              <Loader2 size={24} className="animate-spin text-[var(--color-text-tertiary)]" />
            </div>
          ) : skillsError ? (
            <div
              role="alert"
              className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
            >
              <AlertCircle size={16} />
              <span>{skillsError}</span>
            </div>
          ) : skills.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
              <Wrench size={40} className="mb-3 opacity-40" />
              <p className="text-sm">暂无可用技能</p>
            </div>
          ) : (
            <div className="space-y-4">
              {Array.from(grouped.entries()).map(([category, categorySkills]) => (
                <div key={category}>
                  <h3 className="text-xs font-semibold text-[var(--color-text-tertiary)] uppercase tracking-wider px-2 mb-1.5">
                    {category}
                  </h3>
                  <div className="space-y-1">
                    {categorySkills.map((skill) => (
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
                          </div>
                          {selectedSkillId === skill.id && (
                            <ChevronRight size={14} className="shrink-0 ml-2 text-blue-500" />
                          )}
                        </div>
                      </button>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Right panel: skill detail */}
      <div className="flex-1 flex flex-col overflow-hidden">
        {!selectedSkill ? (
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
              <span className="inline-block mt-2 px-2 py-0.5 text-xs rounded-full bg-[var(--color-bg-secondary)] border border-[var(--color-border)] text-[var(--color-text-tertiary)]">
                {selectedSkill.category}
              </span>
            </div>

            {/* Parameters & Execute */}
            <div className="flex-1 overflow-y-auto p-6">
              <div className="space-y-5 max-w-xl">
                {/* Parameter form */}
                {(selectedSkill.parameters?.length ?? 0) > 0 && (
                  <div className="space-y-4">
                    <h3 className="text-sm font-medium text-[var(--color-text-primary)]">参数</h3>
                    {(selectedSkill.parameters ?? []).map((param) => {
                      const val = paramValues[param.name];
                      if (val === undefined) return null;

                      return (
                        <div key={param.name}>
                          <label className="block text-sm text-[var(--color-text-secondary)] mb-1">
                            {param.name}
                            {param.required && <span className="text-red-500 ml-0.5">*</span>}
                          </label>
                          <ParameterInput param={param} value={val} onChange={handleParamChange} />
                          {param.description && param.type !== 'boolean' && (
                            <p className="text-xs text-[var(--color-text-tertiary)] mt-1">
                              {param.description}
                            </p>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}

                {/* Execute button */}
                <button
                  onClick={handleExecute}
                  disabled={executing}
                  aria-label={executing ? '正在执行技能...' : '执行技能'}
                  className={`flex items-center gap-2 px-5 py-2.5 rounded-lg text-sm font-medium transition-colors ${
                    executing
                      ? 'bg-[var(--color-bg-secondary)] text-[var(--color-text-tertiary)] cursor-not-allowed'
                      : 'bg-blue-600 text-white hover:bg-blue-700 active:bg-blue-800'
                  }`}
                >
                  {executing ? (
                    <>
                      <Loader2 size={16} className="animate-spin" />
                      执行中...
                    </>
                  ) : (
                    <>
                      <Play size={16} />
                      执行
                    </>
                  )}
                </button>

                {/* Execution result */}
                {executionStatus && (
                  <div
                    role="status"
                    aria-live="polite"
                    className={`rounded-lg border p-4 space-y-2 ${
                      executionStatus.status === 'completed'
                        ? 'bg-green-50 dark:bg-green-950 border-green-200 dark:border-green-800'
                        : executionStatus.status === 'failed'
                          ? 'bg-red-50 dark:bg-red-950 border-red-200 dark:border-red-800'
                          : 'bg-blue-50 dark:bg-blue-950 border-blue-200 dark:border-blue-800'
                    }`}
                  >
                    <div className="flex items-center gap-2">
                      {executionStatus.status === 'completed' ? (
                        <CheckCircle2 size={18} className="text-green-600 dark:text-green-400" />
                      ) : executionStatus.status === 'failed' ? (
                        <AlertCircle size={18} className="text-red-600 dark:text-red-400" />
                      ) : (
                        <Loader2
                          size={18}
                          className="animate-spin text-blue-600 dark:text-blue-400"
                        />
                      )}
                      <span
                        className={`text-sm font-medium ${
                          executionStatus.status === 'completed'
                            ? 'text-green-700 dark:text-green-300'
                            : executionStatus.status === 'failed'
                              ? 'text-red-700 dark:text-red-300'
                              : 'text-blue-700 dark:text-blue-300'
                        }`}
                      >
                        {executionStatus.status === 'completed'
                          ? '执行成功'
                          : executionStatus.status === 'failed'
                            ? '执行失败'
                            : '执行中...'}
                      </span>
                    </div>

                    {executionStatus.status === 'completed' && executionStatus.result != null && (
                      <pre className="mt-2 p-3 rounded-md bg-[var(--color-bg-primary)] text-sm text-[var(--color-text-primary)] overflow-x-auto whitespace-pre-wrap">
                        {String(executionStatus.result)}
                      </pre>
                    )}

                    {executionStatus.status === 'failed' && executionStatus.error && (
                      <p className="mt-1 text-sm text-red-600 dark:text-red-400">
                        {executionStatus.error}
                      </p>
                    )}
                  </div>
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
