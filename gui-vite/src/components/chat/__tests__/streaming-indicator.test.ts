import { describe, it, expect } from 'vitest';
import { streamingIndicatorOwner } from '../streaming-indicator';

describe('streamingIndicatorOwner', () => {
  it('returns bubble when thinking is present and content empty', () => {
    expect(streamingIndicatorOwner({ content: '', thinking: '思考中' })).toBe('bubble');
  });

  it('returns list when thinking is empty and content empty', () => {
    expect(streamingIndicatorOwner({ content: '', thinking: null })).toBe('list');
    expect(streamingIndicatorOwner({ content: '', thinking: '' })).toBe('list');
  });

  it('returns null when content is produced (either indicator hides)', () => {
    expect(streamingIndicatorOwner({ content: '正文', thinking: '思考中' })).toBeNull();
    expect(streamingIndicatorOwner({ content: '正文', thinking: null })).toBeNull();
  });
});
