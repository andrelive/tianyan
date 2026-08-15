import type { ModelCapability } from '@/lib/types';

export interface WizardData {
  providerName: string;
  providerEndpoint: string;
  providerApiKey: string;
  modelName: string;
  modelCaps: ModelCapability[];
  dataDir: string;
  vectorDim: number;
  maxTurns: number;
}

export interface StepProps {
  data: WizardData;
  onChange: (updates: Partial<WizardData>) => void;
  onNext: () => void;
  onBack: () => void;
  errors: Record<string, string>;
}
