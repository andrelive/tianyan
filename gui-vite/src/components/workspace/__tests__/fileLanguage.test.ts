/** fileLanguage 纯函数测试（F5）：扩展名 → CodeMirror 语言。 */
import { describe, it, expect } from 'vitest';
import { languageForPath } from '../fileLanguage';

describe('languageForPath', () => {
  it('maps known extensions to language extensions', () => {
    expect(languageForPath('src/main.rs')).not.toBeNull();
    expect(languageForPath('src/app.ts')).not.toBeNull();
    expect(languageForPath('src/app.tsx')).not.toBeNull();
    expect(languageForPath('src/app.js')).not.toBeNull();
    expect(languageForPath('src/app.py')).not.toBeNull();
  });

  it('returns null for unknown extensions', () => {
    expect(languageForPath('README.md')).toBeNull();
    expect(languageForPath('Makefile')).toBeNull();
    expect(languageForPath('noext')).toBeNull();
  });

  it('is case-insensitive on the extension', () => {
    expect(languageForPath('X.RS')).not.toBeNull();
    expect(languageForPath('x.TSX')).not.toBeNull();
  });
});
