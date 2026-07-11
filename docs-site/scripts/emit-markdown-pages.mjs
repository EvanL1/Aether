import { fileURLToPath } from 'node:url';
import path from 'node:path';
import fs from 'node:fs/promises';
import fg from 'fast-glob';
import { computeSlug } from './slug.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const DOCS_SITE_ROOT = path.resolve(__dirname, '..');
const CONTENT_DIR = path.join(DOCS_SITE_ROOT, 'src', 'content', 'docs');
const DIST_DIR = path.join(DOCS_SITE_ROOT, 'dist');

// Maps a slug (as produced by computeSlug) to a filesystem path inside
// dist/ — distinct from slug.mjs's slugToSitePath, which maps a slug to a
// URL. '' (the root index page) becomes dist/index.md; everything else
// becomes dist/<slug>.md, mirroring the directory Starlight emits
// dist/<slug>/index.html into.
export function slugToOutputRelPath(slug) {
  return slug === '' ? 'index.md' : `${slug}.md`;
}

// Guards against the glob silently matching nothing (e.g. this script run
// standalone before `npm run sync` has populated src/content/docs/, or a
// split build/deploy pipeline step that skips sync) — without this, the
// script would print "wrote 0 file(s)" and exit 0, silently dropping the
// per-page-markdown feature site-wide behind a green build log. Mirrors
// sync-content.mjs's empty-manifest-pattern guard.
export function assertFilesFound(files) {
  if (files.length === 0) {
    throw new Error(
      'emit-markdown-pages: no markdown files found under src/content/docs/ — did you run `npm run sync` first?'
    );
  }
}

// Pure: groups (relPath, outRelPath) pairs by outRelPath and returns one
// entry per output path with more than one source. computeSlug lowercases
// and slugifies each path segment, which is lossier than the source paths
// themselves (e.g. concepts/Architecture.md and concepts/architecture.md
// would both compute to concepts/architecture.md) — no such collision
// exists in today's content, but nothing prevents one from being introduced
// later. Mirrors sync-content.mjs's findCollisions: collect every problem
// and throw once before any write, rather than one file silently clobbering
// another mid-Promise.all.
export function findOutputCollisions(pairs) {
  const sourcesByOutput = new Map();
  for (const [relPath, outRelPath] of pairs) {
    if (!sourcesByOutput.has(outRelPath)) {
      sourcesByOutput.set(outRelPath, []);
    }
    sourcesByOutput.get(outRelPath).push(relPath);
  }

  const collisions = [];
  for (const [outRelPath, sources] of sourcesByOutput) {
    if (sources.length > 1) {
      collisions.push({ outRelPath, sources });
    }
  }
  return collisions;
}

async function main() {
  const files = await fg('**/*.md', { cwd: CONTENT_DIR, onlyFiles: true });
  assertFilesFound(files);

  const pairs = files.map((relPath) => [relPath, slugToOutputRelPath(computeSlug(relPath))]);

  const collisions = findOutputCollisions(pairs);
  if (collisions.length > 0) {
    const details = collisions
      .map(({ outRelPath, sources }) => `  ${outRelPath} <- ${sources.join(', ')}`)
      .join('\n');
    throw new Error(`emit-markdown-pages: output path collision(s) detected:\n${details}`);
  }

  await Promise.all(
    pairs.map(async ([relPath, outRelPath]) => {
      const outAbsPath = path.join(DIST_DIR, outRelPath);
      const content = await fs.readFile(path.join(CONTENT_DIR, relPath), 'utf8');

      await fs.mkdir(path.dirname(outAbsPath), { recursive: true });
      await fs.writeFile(outAbsPath, content, 'utf8');
    })
  );

  console.log(`emit-markdown-pages: wrote ${files.length} raw markdown file(s) to dist/`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((err) => {
    console.error(err);
    process.exitCode = 1;
  });
}
