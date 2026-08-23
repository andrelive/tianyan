/**
 * 模型/能力目录切片（modelSlice）—— 聊天模型选择、思考强度档位、
 * 聊天模型目录与技能目录（均为从后端加载的目录数据 + 用户选择）。
 */

import type { StateCreator } from 'zustand';
import type { ModelInfo, Skill } from '@/lib/types';

export interface ModelSlice {
  // Model
  selectedModel: string | null;
  setModel: (model: string | null) => void;

  // 会话级思考强度档位（对话时选择，随每次请求下发；值为当前模型声明的档位）
  thinkingEffort: string;
  setThinkingEffort: (effort: string) => void;

  // 聊天模型目录（含每模型思考档位；ThinkingSelect 按当前模型档位渲染）
  chatModels: ModelInfo[];
  setChatModels: (models: ModelInfo[]) => void;

  // Skills
  skills: Skill[];
  setSkills: (skills: Skill[]) => void;
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

  // Skills
  skills: [],
  setSkills: (skills) => set({ skills }),
});
