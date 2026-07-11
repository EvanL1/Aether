import { describe, expect, it } from 'vitest';
import { computeSlug } from './slug.mjs';
import {
  assertFilesFound,
  findOutputCollisions,
  slugToOutputRelPath,
} from './emit-markdown-pages.mjs';

describe('computeSlug (reused from slug.mjs)', () => {
  // Only a lowercasing sanity check lives here — index.md/index-segment
  // cases are already covered by sync-content.test.mjs, which owns slug.mjs.
  it('strips the extension and lowercases', () => {
    expect(computeSlug('concepts/Architecture.md')).toBe('concepts/architecture');
  });
});

describe('slugToOutputRelPath', () => {
  it('maps the empty slug to index.md', () => {
    expect(slugToOutputRelPath('')).toBe('index.md');
  });

  it('appends .md to a normal slug', () => {
    expect(slugToOutputRelPath('guides/getting-started')).toBe('guides/getting-started.md');
  });
});

describe('assertFilesFound', () => {
  it('throws when zero files are found', () => {
    // Reproduced directly: running this script standalone (outside `npm run
    // build`, e.g. before `npm run sync` populated src/content/docs/, or in
    // a misconfigured CI step) globs zero files and, without this guard,
    // would silently write nothing and exit 0.
    expect(() => assertFilesFound([])).toThrow(/no markdown files found/);
  });

  it('does not throw when files are found', () => {
    expect(() => assertFilesFound(['index.md'])).not.toThrow();
  });
});

describe('findOutputCollisions', () => {
  it('returns no collisions when every computed output path is unique', () => {
    const pairs = [
      ['index.md', 'index.md'],
      ['concepts/architecture.md', 'concepts/architecture.md'],
      ['guides/getting-started.md', 'guides/getting-started.md'],
    ];
    expect(findOutputCollisions(pairs)).toEqual([]);
  });

  it('flags two sources whose computed output paths collide', () => {
    // computeSlug lowercases and slugifies every path segment, which is
    // lossier than the source paths themselves — a mixed-case and a
    // lowercase source in the same directory would both compute to the same
    // output path and silently clobber one another mid-write without this
    // guard. No such collision exists in today's content, but this script
    // has no way to guarantee one won't be introduced later.
    const pairs = [
      ['concepts/Architecture.md', 'concepts/architecture.md'],
      ['concepts/architecture.md', 'concepts/architecture.md'],
    ];
    const collisions = findOutputCollisions(pairs);
    expect(collisions).toHaveLength(1);
    expect(collisions[0].outRelPath).toBe('concepts/architecture.md');
    expect(collisions[0].sources).toEqual([
      'concepts/Architecture.md',
      'concepts/architecture.md',
    ]);
  });
});
