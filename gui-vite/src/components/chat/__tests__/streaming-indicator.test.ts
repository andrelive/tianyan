import { describe, it, expect } from 'vitest';
import { streamingIndicatorOwner } from '../streaming-indicator';

describe('streamingIndicatorOwner', () => {
  it('returns bubble when thinking is present and text empty', () => {
    expect(streamingIndicatorOwner({ segments: [], thinking: '思考中' })).toBe('bubble');
  });

  it('returns list when thinking is empty and text empty', () => {
    expect(streamingIndicatorOwner({ segments: [], thinking: null })).toBe('list');
    expect(streamingIndicatorOwner({ segments: [], thinking: '' })).toBe('list');
  });

  it('returns null when text is produced (either indicator hides)', () => {
    expect(
      streamingIndicatorOwner({ segments: [{ type: 'text', text: '正文' }], thinking: '思考中' }),
    ).toBeNull();
    expect(
      streamingIndicatorOwner({ segments: [{ type: 'text', text: '正文' }], thinking: null }),
    ).toBeNull();
  });
});
