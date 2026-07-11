import { slug as githubSlug } from 'github-slugger';

// Mirrors Starlight's own slug derivation exactly. Astro's content loader
// (node_modules/astro/dist/content/utils.js, getContentEntryIdAndSlug)
// slugifies each path segment independently with github-slugger's stateless
// `slug()` function (not the default-exported GithubSlugger class, which
// tracks occurrences across calls and would append -1/-2 suffixes — we want
// the same segment to always slug the same way), then joins the slugged
// segments with '/' and strips a trailing '/index'. This reproduces that
// algorithm (plus Starlight's root-index special case) so a computed URL
// always matches what Starlight actually serves the page at. Shared between
// sync-content.mjs (link rewriting) and any future per-page markdown output
// (Task 5).

export function computeSlug(destRelPath) {
  const withoutExt = destRelPath.replace(/\.(md|mdx)$/i, '');
  const segments = withoutExt.split('/').filter((s) => s.length > 0);
  const slugged = segments.map((s) => githubSlug(s));
  let slug = slugged.join('/');
  if (slug.endsWith('/index')) slug = slug.slice(0, -'/index'.length);
  if (slug.toLowerCase() === 'index' || slug === '') slug = '';
  return slug.toLowerCase();
}

export function slugToSitePath(slug) {
  return slug === '' ? '/' : `/${slug}/`;
}
