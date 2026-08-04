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

  /* ── Model (step 1) ── */
  const [providerName, setProviderName] = useState('');
  const [providerEndpoint, setProviderEndpoint] = useState('');
  const [providerApiKey, setProviderApiKey] = useState('');
  const [modelName, setModelName] = useState('');
  const [modelCaps, setModelCaps] = useState<ModelCapability[]>(['chat' as ModelCapability]);

  /* ── Data (step 2) ── */
  const [dataDir, setDataDir] = useState('');
  const [vectorDim, setVectorDim] = useState(1536);

  /* ── Agent (step 3) ── */
  const [enableSkills, setEnableSkills] = useState(true);
  const [enableMemory, setEnableMemory] = useState(true);
  const [streamResponses, setStreamResponses] = useState(true);
  const [enableThinking, setEnableThinking] = useState(false);
  const [maxTurns, setMaxTurns] = useState(200);

  /* ── Build WizardData from state ── */

  const wizardData: WizardData = {
    providerName,
    providerEndpoint,
    providerApiKey,
    modelName,
    modelCaps,
    dataDir,
    vectorDim,
    enableSkills,
    enableMemory,
    streamResponses,
    enableThinking,
    maxTurns,
  };

  /* ── Handle partial updates from step components ── */

  const handleChange = useCallback((updates: Partial<WizardData>) => {
    if (updates.providerName !== undefined) setProviderName(updates.providerName);
    if (updates.providerEndpoint !== undefined) setProviderEndpoint(updates.providerEndpoint);
    if (updates.providerApiKey !== undefined) setProviderApiKey(updates.providerApiKey);
    if (updates.modelName !== undefined) setModelName(updates.modelName);
    if (updates.modelCaps !== undefined) setModelCaps(updates.modelCaps);
    if (updates.dataDir !== undefined) setDataDir(updates.dataDir);
    if (updates.vectorDim !== undefined) setVectorDim(updates.vectorDim);
    if (updates.enableSkills !== undefined) setEnableSkills(updates.enableSkills);
    if (updates.enableMemory !== undefined) setEnableMemory(updates.enableMemory);
    if (updates.streamResponses !== undefined) setStreamResponses(updates.streamResponses);
    if (updates.enableThinking !== undefined) setEnableThinking(updates.enableThinking);
    if (updates.maxTurns !== undefined) setMaxTurns(updates.maxTurns);
  }, []);

  /* ── Navigation ── */
  const canGoNext = useCallback((): boolean => {
    switch (step) {
      case 0:
        return true;
      case 1:
        return (
          providerName.trim().length > 0 &&
          providerEndpoint.trim().length > 0 &&
          providerApiKey.trim().length > 0 &&
          modelName.trim().length > 0
        );
      case 2:
        return dataDir.trim().length > 0;
      case 3:
        return true;
      case 4:
        return true;
      default:
        return false;
    }
  }, [step, providerName, providerEndpoint, providerApiKey, modelName, dataDir]);

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
      enable_skills: enableSkills,
      enable_memory: enableMemory,
      stream_responses: streamResponses,
      enable_thinking: enableThinking,
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
  }, [
    providerName,
    providerEndpoint,
    providerApiKey,
    modelName,
    modelCaps,
    dataDir,
    vectorDim,
    enableSkills,
    enableMemory,
    streamResponses,
    enableThinking,
    maxTurns,
    showToast,
    setConfigured,
  ]);

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
