/**
 * 模型/能力目录切片（modelSlice）—— 聊天模型选择、思考强度档位、
 * 聊天模型目录（均为从后端加载的目录数据 + 用户选择）。
 *
 * 技能目录为页面级数据（仅 SkillsPanel 消费），留在组件局部 state，不进全局 store。
 */

import type { StateCreator } from 'zustand';
import type { ModelInfo } from '@/lib/types';

/** 当前选中的聊天模型（provider+model 复合引用；同名模型跨 provider 的唯一标识）。 */
export interface SelectedModel {
  provider: string;
  model: string;
}

export interface ModelSlice {
  // Model
  selectedModel: SelectedModel | null;
  setModel: (model: SelectedModel | null) => void;

  // 会话级思考强度档位（对话时选择，随每次请求下发；值为当前模型声明的档位）
  thinkingEffort: string;
  setThinkingEffort: (effort: string) => void;

  // 聊天模型目录（含每模型思考档位；ThinkingSelect 按当前模型档位渲染）
  chatModels: ModelInfo[];
  setChatModels: (models: ModelInfo[]) => void;
}

export const createModelSlice: StateCreator<ModelSlice, [], [], ModelSlice> = (set) => ({
  // Model
  selectedModel: null,
  setModel: (model) => set({ selectedModel: model }),

  // 会话级思考强度（默认关闭；非 off 时对支持思考的模型生效）
  thinkingEffort: 'off',
  setThinkingEffort: (effort) => set({ thinkingEffort: effort }),
  chatModels: [],
  setChatModels: (models) => set({ chatModels: models }),
});
