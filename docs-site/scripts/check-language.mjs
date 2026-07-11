import { fileURLToPath } from 'node:url';
import path from 'node:path';
import fs from 'node:fs/promises';
import fg from 'fast-glob';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const CONTENT_DIR = path.resolve(__dirname, '..', 'src', 'content', 'docs');
const CJK_PATTERN = /\p{Script=Han}|\p{Script=Hiragana}|\p{Script=Katakana}|\p{Script=Hangul}/u;

export function findCjkOccurrences(sourcePath, content) {
  return content
    .split('\n')
    .map((text, index) => ({ path: sourcePath, line: index + 1, text }))
    .filter(({ text }) => CJK_PATTERN.test(text));
}

export function assertEnglishOnly(documents) {
  const occurrences = documents.flatMap(({ path: sourcePath, content }) =>
    findCjkOccurrences(sourcePath, content)
  );
  if (occurrences.length === 0) return;

  const details = occurrences
    .map(({ path: sourcePath, line, text }) => `  ${sourcePath}:${line}: ${text.trim()}`)
    .join('\n');
  throw new Error(`Published documentation must be English-only:\n${details}`);
}

/* v8 ignore start -- filesystem orchestration is exercised by npm run build. */
async function main() {
  const files = (await fg('**/*.md', { cwd: CONTENT_DIR, onlyFiles: true })).sort();
  const documents = await Promise.all(
    files.map(async (sourcePath) => ({
      path: sourcePath,
      content: await fs.readFile(path.join(CONTENT_DIR, sourcePath), 'utf8'),
    }))
  );
  assertEnglishOnly(documents);
  console.log(`check-language: verified ${documents.length} English-only documents`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
/* v8 ignore stop */
