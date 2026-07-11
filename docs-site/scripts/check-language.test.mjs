import { describe, expect, it } from 'vitest';
import { assertEnglishOnly, findCjkOccurrences } from './check-language.mjs';

describe('findCjkOccurrences', () => {
  it('reports the source path, line, and offending text', () => {
    expect(findCjkOccurrences('guide.md', '# Guide\n\n这是中文。\n')).toEqual([
      { path: 'guide.md', line: 3, text: '这是中文。' },
    ]);
  });

  it('accepts English Markdown with Unicode punctuation', () => {
    expect(findCjkOccurrences('guide.md', '# Guide — Aether\n\nIt’s agent-native.\n')).toEqual([]);
  });
});

describe('assertEnglishOnly', () => {
  it('throws one actionable error containing every offending file', () => {
    expect(() =>
      assertEnglishOnly([
        { path: 'first.md', content: 'English.\n中文。\n' },
        { path: 'second.md', content: '# 标题\n' },
      ])
    ).toThrow(/first\.md:2[\s\S]*second\.md:1/);
  });

  it('does not throw for an English-only publication set', () => {
    expect(() =>
      assertEnglishOnly([{ path: 'guide.md', content: '# Guide\n\nEnglish only.\n' }])
    ).not.toThrow();
  });
});
