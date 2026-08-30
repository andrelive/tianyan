import { useState, useCallback } from 'react';
import {
  Check,
  ChevronLeft,
  ChevronRight,
  Bot,
  Database,
  Cpu,
  Globe,
  Rocket,
  Loader2,
} from 'lucide-react';

import { useAppStore } from '@/lib/store';
import { saveConfig } from '@/lib/api-client';
import { toBackendConfig, emptyConfigState } from '@/lib/config-transform';
import { toErrorMessage } from '@/lib/errors';
import type { ConfigState, ModelPreferencesState } from '@/lib/types';

import type { StepProps } from './steps/wizard.types';
import WelcomeStep from './steps/WelcomeStep';
import ModelStep from './steps/ModelStep';
import DataStep from './steps/DataStep';
import AgentStep from './steps/AgentStep';
import WebStep from './steps/WebStep';
import ConfirmStep from './steps/ConfirmStep';
import StepIndicator from './StepIndicator';

const STEPS = [
  { id: 'welcome', label: '欢迎', icon: Rocket },
  { id: 'model', label: '模型配置', icon: Cpu },
  { id: 'data', label: '数据存储', icon: Database },
  { id: 'web', label: 'Web 搜索', icon: Globe },
  { id: 'agent', label: 'Agent 行为', icon: Bot },
  { id: 'confirm', label: '确认', icon: Check },
];

/** 能力标签 → preferences 绑定（向导收尾唯一派生点；与 ModelsTab 同规则）。 */
function derivePreferences(config: ConfigState): ModelPreferencesState {
  const p = config.providers[0];
  const m = p?.models[0];
  if (!p || !m) return { chat: null, embedding: null, vision: null };
  const prefs: ModelPreferencesState = { chat: null, embedding: null, vision: null };
  const ref = { provider: p.name, model: m.name };
  if (m.capabilities.includes('chat')) prefs.chat = ref;
  if (
    m.capabilities.includes('text-embedding') ||
    m.capabilities.includes('multimodal-embedding')
  ) {
    prefs.embedding = ref;
  }
  if (m.capabilities.includes('vision')) prefs.vision = ref;
  return prefs;
}

/* ─────── ConfigWizard ─────── */

export default function ConfigWizard() {
  const showToast = useAppStore((s) => s.showToast);
  const setConfigured = useAppStore((s) => s.setConfigured);

  const [step, setStep] = useState(0);
  const [submitting, setSubmitting] = useState(false);

  /* F4：向导直接编辑 ConfigState（与设置面板同源），不再维护平行 WizardData */
  const [config, setConfig] = useState<ConfigState>(() => ({
    ...emptyConfigState(),
    // 向导语义默认：与原 WizardData 默认一致（emptyConfigState 为产品默认 20）
    max_turns: 200,
  }));

  /* ── Handle partial updates from step components ── */

  const handleChange = useCallback((updates: Partial<ConfigState>) => {
    setConfig((prev) => ({ ...prev, ...updates }));
  }, []);

  /* ── Navigation ── */
  const canGoNext = useCallback((): boolean => {
    const p = config.providers[0];
    switch (step) {
      case 0:
        return true;
      case 1:
        return (
          !!p &&
          p.name.trim().length > 0 &&
          p.endpoint.trim().length > 0 &&
          p.api_key.trim().length > 0 &&
          (p.models[0]?.name ?? '').trim().length > 0
        );
      case 2:
        return config.data_dir.trim().length > 0;
      case 3:
        return true;
      case 4:
        return true;
      case 5:
        return true;
      default:
        return false;
    }
  }, [step, config]);

  const handleNext = useCallback(() => {
    if (!canGoNext()) {
      showToast('请填写必填字段', 'info');
      return;
    }
    setStep((s) => Math.min(s + 1, 5));
  }, [canGoNext, showToast]);

  const handlePrev = useCallback(() => {
    setStep((s) => Math.max(s - 1, 0));
  }, []);

  /* ── Finish：能力标签 → preferences 派生（唯一派生点）后保存 ── */
  const handleFinish = useCallback(async () => {
    const finalConfig: ConfigState = {
      ...config,
      preferences: derivePreferences(config),
    };
    setSubmitting(true);
    try {
      const payload = toBackendConfig(finalConfig);
      await saveConfig(payload);
      showToast('配置完成！正在启动天演...', 'success');
      setConfigured(true);
    } catch (err: unknown) {
      const msg = toErrorMessage(err, '配置保存失败');
      showToast(`配置保存失败: ${msg}`, 'error');
      setSubmitting(false);
    }
  }, [config, showToast, setConfigured]);

  /* ─────── Render ─────── */

  const commonStepProps: StepProps = {
    data: config,
    onChange: handleChange,
    onNext: handleNext,
    onBack: handlePrev,
    errors: {},
  };

  return (
    <div className="h-screen flex flex-col bg-[var(--color-bg-primary)]">
      {/* Header + Step indicator */}
      <div className="px-6 pt-6 pb-4 border-b border-[var(--color-border)]">
        <StepIndicator current={step} steps={STEPS} />
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-lg mx-auto p-6">
          {step === 0 && <WelcomeStep {...commonStepProps} />}
          {step === 1 && <ModelStep {...commonStepProps} />}
          {step === 2 && <DataStep {...commonStepProps} />}
          {step === 3 && <WebStep {...commonStepProps} />}
          {step === 4 && <AgentStep {...commonStepProps} />}
          {step === 5 && <ConfirmStep {...commonStepProps} submitting={submitting} />}
        </div>
      </div>

      {/* Footer navigation */}
      <div className="px-6 py-4 border-t border-[var(--color-border)] flex items-center justify-between bg-[var(--color-bg-secondary)]">
        <div>
          {step > 0 && (
            <button
              onClick={handlePrev}
              className="flex items-center gap-1.5 px-4 py-2 text-sm rounded-md border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors"
            >
              <ChevronLeft size={16} />
              上一步
            </button>
          )}
        </div>

        <div className="text-xs text-[var(--color-text-tertiary)]">
          {step + 1} / {STEPS.length}
        </div>

        <div>
          {step < 5 ? (
            <button
              onClick={handleNext}
              disabled={!canGoNext()}
              className="flex items-center gap-1.5 px-5 py-2 text-sm font-medium rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              下一步
              <ChevronRight size={16} />
            </button>
          ) : (
            <button
              onClick={() => void handleFinish()}
              disabled={submitting}
              className="flex items-center gap-1.5 px-5 py-2 text-sm font-medium rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors"
            >
              {submitting ? <Loader2 size={16} className="animate-spin" /> : <Check size={16} />}
              {submitting ? '保存中...' : '完成配置'}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
