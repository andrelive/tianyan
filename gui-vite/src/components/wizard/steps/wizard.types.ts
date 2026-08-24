import { emptyProvider } from '@/lib/config-transform';
import type { ConfigState, ModelCapability, ProviderConfigState } from '@/lib/types';

/**
 * 向导步骤 props（F4：向导直接编辑 ConfigState，与设置面板同源——
 * 不再维护平行 WizardData 与收尾重建）。
 */
export interface StepProps {
  data: ConfigState;
  onChange: (updates: Partial<ConfigState>) => void;
  onNext: () => void;
  onBack: () => void;
  errors: Record<string, string>;
}

/** 向导只编辑第一个提供商；缺失时返回空默认（onChange 时须写回 providers[0]）。 */
export function firstProvider(config: ConfigState): ProviderConfigState {
  return config.providers[0] ?? emptyProvider();
}

/** 第一个提供商的第一个模型（缺失时返回默认形态）。 */
export function firstModel(config: ConfigState): { name: string; capabilities: ModelCapability[] } {
  const p = firstProvider(config);
  return p.models[0] ?? { name: '', capabilities: ['chat'] };
}
