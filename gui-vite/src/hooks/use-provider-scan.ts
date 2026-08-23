/**
 * useProviderScan —— 模型发现扫描状态机（ModelsTab 扫描五元组收敛点）。
 *
 * 每个 provider 独立维护 { protocol, scanning, models, picked, error }；
 * 动作：scan（发起扫描，成功后按已配置模型去重并默认勾选新模型）、
 * togglePicked、adoptPicked（返回待采纳模型并清空）、setScanProtocol。
 * 纯 UI 状态（不持久化）；状态机与组件解耦，可脱离 ModelsTab 直接单测。
 */

import { useCallback, useRef, useState } from 'react';
import { scanProviderModels } from '@/lib/api-client';
import { toErrorMessage } from '@/lib/errors';
import type { DiscoveredModelInfo, ProviderProtocol } from '@/lib/types';

export interface ProviderScanState {
  protocol: ProviderProtocol;
  scanning: boolean;
  models: DiscoveredModelInfo[];
  picked: string[];
  error: string | null;
}

const EMPTY_SCAN: ProviderScanState = {
  protocol: 'openai',
  scanning: false,
  models: [],
  picked: [],
  error: null,
};

export function useProviderScan(getKnownNames: (providerIndex: number) => Set<string>) {
  const [scanState, setScanState] = useState<Record<number, ProviderScanState>>({});
  const getKnownNamesRef = useRef(getKnownNames);
  getKnownNamesRef.current = getKnownNames;
  // 最新状态镜像：adoptPicked 需要在事件处理器内同步读取当前状态
  const scanStateRef = useRef(scanState);
  scanStateRef.current = scanState;

  const setScanProtocol = useCallback((pi: number, protocol: ProviderProtocol) => {
    setScanState((prev) => ({
      ...prev,
      [pi]: prev[pi] ? { ...prev[pi], protocol } : { ...EMPTY_SCAN, protocol },
    }));
  }, []);

  const scan = useCallback(
    async (pi: number, endpoint: string, protocol: ProviderProtocol, providerName: string) => {
      setScanState((prev) => ({
        ...prev,
        [pi]: { ...(prev[pi] ?? EMPTY_SCAN), scanning: true, error: null },
      }));
      try {
        const resp = await scanProviderModels(endpoint, protocol, providerName);
        setScanState((prev) => {
          const base = prev[pi] ?? EMPTY_SCAN;
          const known = getKnownNamesRef.current(pi);
          const models = resp.success ? resp.models : [];
          // 新模型默认勾选；已配置的模型排除在可勾选外
          const picked = models.filter((m) => !known.has(m.name)).map((m) => m.name);
          return {
            ...prev,
            [pi]: {
              ...base,
              scanning: false,
              models,
              picked,
              error: resp.success ? null : (resp.error ?? '扫描失败'),
            },
          };
        });
      } catch (err: unknown) {
        const msg = toErrorMessage(err, '扫描请求失败');
        setScanState((prev) => ({
          ...prev,
          [pi]: { ...(prev[pi] ?? EMPTY_SCAN), scanning: false, error: msg },
        }));
      }
    },
    [],
  );

  const togglePicked = useCallback((pi: number, name: string) => {
    setScanState((prev) => {
      const st = prev[pi];
      if (!st) return prev;
      const picked = st.picked.includes(name)
        ? st.picked.filter((n) => n !== name)
        : [...st.picked, name];
      return { ...prev, [pi]: { ...st, picked } };
    });
  }, []);

  const adoptPicked = useCallback((pi: number): DiscoveredModelInfo[] => {
    const st = scanStateRef.current[pi];
    if (!st) return [];
    const models = st.models.filter((m) => st.picked.includes(m.name));
    if (models.length === 0) return [];
    setScanState((prev) => {
      const cur = prev[pi];
      if (!cur) return prev;
      return { ...prev, [pi]: { ...cur, models: [], picked: [], error: null } };
    });
    return models;
  }, []);

  return { scanState, setScanProtocol, scan, togglePicked, adoptPicked };
}
