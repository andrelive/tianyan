import { describe, it, expect } from 'vitest';
import { toErrorMessage } from '../errors';
import { ApiError } from '../api-client';

describe('toErrorMessage', () => {
  it('extracts message from Error instances', () => {
    expect(toErrorMessage(new Error('boom'))).toBe('boom');
  });

  it('falls back to the default text for non-Error values', () => {
    expect(toErrorMessage('boom')).toBe('操作失败');
    expect(toErrorMessage(null)).toBe('操作失败');
    expect(toErrorMessage(undefined)).toBe('操作失败');
    expect(toErrorMessage({ weird: true })).toBe('操作失败');
  });

  it('uses the caller-supplied fallback', () => {
    expect(toErrorMessage('boom', '扫描请求失败')).toBe('扫描请求失败');
    expect(toErrorMessage(new Error('boom'), '扫描请求失败')).toBe('boom');
  });

  it('works with ApiError (语义谓词携带的 message)', () => {
    expect(toErrorMessage(new ApiError('角色不存在', 404))).toBe('角色不存在');
  });
});
