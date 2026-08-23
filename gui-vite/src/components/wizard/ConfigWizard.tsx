import { useState, useCallback } from 'react';
import {
  Check,
  ChevronLeft,
  ChevronRight,
  Bot,
  Database,
  Cpu,
  Rocket,
  Loader2,
} from 'lucide-react';

import { useAppStore } from '@/lib/store';
import { apiPut } from '@/lib/api-client';
import { toBackendConfig, emptyConfigState } from '@/lib/config-transform';
import type { ConfigState, ModelCapability } from '@/lib/types';

import type { WizardData } from './steps/wizard.types';
import WelcomeStep from './steps/WelcomeStep';
import ModelStep from './steps/ModelStep';
import DataStep from './steps/DataStep';
import AgentStep from './steps/AgentStep';
import ConfirmStep from './steps/ConfirmStep';
import StepIndicator from './StepIndicator';

const STEPS = [
  { id: 'welcome', label: '欢迎', icon: Rocket },
  { id: 'model', label: '模型配置', icon: Cpu },
  { id: 'data', label: '数据存储', icon: Database },
  { id: 'agent', label: 'Agent 行为', icon: Bot },
  { id: 'confirm', label: '确认', icon: Check },
];

/* ─────── ConfigWizard ─────── */

export default function ConfigWizard() {
  const showToast = useAppStore((s) => s.showToast);
  const setConfigured = useAppStore((s) => s.setConfigured);

  const [step, setStep] = useState(0);
  const [submitting, setSubmitting] = useState(false);

  /* 单一 WizardData 状态（E6：8 个 useState + 扇出合并收敛为一个对象——
     对象即真相源，handleChange 收敛为 spread 合并） */
  const [wizardData, setWizardData] = useState<WizardData>({
    providerName: '',
    providerEndpoint: '',
    providerApiKey: '',
    modelName: '',
    modelCaps: ['chat' as ModelCapability],
    dataDir: '',
    vectorDim: 1536,
    maxTurns: 200,
  });

  /* ── Handle partial updates from step components ── */

  const handleChange = useCallback((updates: Partial<WizardData>) => {
    setWizardData((prev) => ({ ...prev, ...updates }));
  }, []);

  /* ── Navigation ── */
  const canGoNext = useCallback((): boolean => {
    switch (step) {
      case 0:
        return true;
      case 1:
        return (
          wizardData.providerName.trim().length > 0 &&
          wizardData.providerEndpoint.trim().length > 0 &&
          wizardData.providerApiKey.trim().length > 0 &&
          wizardData.modelName.trim().length > 0
        );
      case 2:
        return wizardData.dataDir.trim().length > 0;
      case 3:
        return true;
      case 4:
        return true;
      default:
        return false;
    }
  }, [step, wizardData]);

  const handleNext = useCallback(() => {
    if (!canGoNext()) {
      showToast('请填写必填字段', 'info');
      return;
    }
    setStep((s) => Math.min(s + 1, 4));
  }, [canGoNext, showToast]);

  const handlePrev = useCallback(() => {
    setStep((s) => Math.max(s - 1, 0));
  }, []);

  /* ── Finish ── */
  const handleFinish = useCallback(async () => {
    // Build ConfigState from wizard state
    const {
      providerName,
      providerEndpoint,
      providerApiKey,
      modelName,
      modelCaps,
      dataDir,
      vectorDim,
      maxTurns,
    } = wizardData;
    const config: ConfigState = {
      ...emptyConfigState(),
      providers: [],
      preferences: {
        chat: null,
        embedding: null,
        vision: null,
      },
      data_dir: dataDir.trim(),
      vector_dimension: vectorDim,
      max_turns: maxTurns,
    };

    // Add provider with one model if fields are filled
    if (providerName.trim() && providerEndpoint.trim()) {
      const modelCapsToUse: ModelCapability[] = modelCaps.length > 0 ? modelCaps : ['chat'];
      config.providers = [
        {
          name: providerName.trim(),
          endpoint: providerEndpoint.trim(),
          api_key: providerApiKey.trim(),
          models: [
            {
              name: modelName.trim(),
              capabilities: modelCapsToUse,
            },
          ],
          timeout: 60,
          enabled: true,
          is_local: false,
          headers: {},
        },
      ];

      // Set preferences: if model has chat cap, use it as default chat model
      if (modelCapsToUse.includes('chat')) {
        config.preferences.chat = {
          provider: providerName.trim(),
          model: modelName.trim(),
        };
      }
      if (
        modelCapsToUse.includes('text-embedding') ||
        modelCapsToUse.includes('multimodal-embedding')
      ) {
        config.preferences.embedding = {
          provider: providerName.trim(),
          model: modelName.trim(),
        };
      }
      if (modelCapsToUse.includes('vision')) {
        config.preferences.vision = {
          provider: providerName.trim(),
          model: modelName.trim(),
        };
      }
    }

    setSubmitting(true);
    try {
      const payload = toBackendConfig(config);
      await apiPut<{ success: boolean; message: string }>('/config', payload);
      showToast('配置完成！正在启动天演...', 'success');
      setConfigured(true);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : '配置保存失败';
      showToast(`配置保存失败: ${msg}`, 'error');
      setSubmitting(false);
    }
  }, [wizardData, showToast, setConfigured]);

  /* ─────── Render ─────── */

  const commonStepProps = {
    data: wizardData,
    onChange: handleChange,
    onNext: handleNext,
    onBack: handlePrev,
    errors: {} as Record<string, string>,
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
          {step === 3 && <AgentStep {...commonStepProps} />}
          {step === 4 && <ConfirmStep {...commonStepProps} submitting={submitting} />}
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
          {step < 4 ? (
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
              onClick={handleFinish}
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
