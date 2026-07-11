import { describe, expect, it } from 'vitest';
import worker from './entry.js';

const files = new Map([
  ['/index.html', '<!doctype html><h1>Aether Documentation</h1>'],
  ['/agent-quickstart/index.html', '<!doctype html><h1>Agent Quickstart</h1>'],
  ['/index.md', '# Aether\n'],
  ['/agent-quickstart.md', '# Agent Quickstart\n'],
  ['/llms.txt', '# Aether\n'],
]);

function environment(options = {}) {
  return {
    ASSETS: {
      async fetch(request) {
        if (options.throwOnFetch) throw new Error('asset binding unavailable');
        const url = new URL(request.url);
        let assetPath = url.pathname;
        if (assetPath === '/') assetPath = '/index.html';
        if (assetPath.endsWith('/')) assetPath += 'index.html';
        const content = files.get(assetPath);
        if (content === undefined) return new Response('missing', { status: 404 });
        const contentType = assetPath.endsWith('.html') ? 'text/html' : 'text/plain';
        return new Response(request.method === 'HEAD' ? null : content, {
          headers: { 'Content-Type': contentType },
        });
      },
    },
  };
}

function run(path, init, options) {
  return worker.fetch(new Request(`https://example.com${path}`, init), environment(options));
}

describe('dual-mode Worker in the Node unit-test runtime', () => {
  it('serves HTML by default and Markdown on explicit request', async () => {
    const html = await run('/agent-quickstart/');
    const markdown = await run('/agent-quickstart/', {
      headers: { Accept: 'text/markdown' },
    });

    expect(html.headers.get('Content-Type')).toContain('text/html');
    expect(await html.text()).toContain('<h1>Agent Quickstart</h1>');
    expect(markdown.headers.get('Content-Type')).toBe('text/markdown; charset=utf-8');
    expect(await markdown.text()).toContain('# Agent Quickstart');
  });

  it('serves direct Markdown and text indexes with distinct content types', async () => {
    const markdown = await run('/agent-quickstart.md');
    const index = await run('/llms.txt');

    expect(markdown.headers.get('Content-Type')).toBe('text/markdown; charset=utf-8');
    expect(index.headers.get('Content-Type')).toBe('text/plain; charset=utf-8');
  });

  it('returns plain-text protocol and Markdown lookup errors', async () => {
    const unsupported = await run('/agent-quickstart', { method: 'POST' });
    const missing = await run('/missing', {
      headers: { Accept: 'text/markdown' },
    });

    expect(unsupported.status).toBe(405);
    expect(unsupported.headers.get('Allow')).toBe('GET, HEAD');
    expect(missing.status).toBe(404);
    expect(missing.headers.get('Cache-Control')).toBe('no-store');
  });

  it('returns bodyless HTML and Markdown responses to HEAD', async () => {
    const html = await run('/', { method: 'HEAD' });
    const markdown = await run('/', {
      method: 'HEAD',
      headers: { Accept: 'text/markdown' },
    });

    expect(html.headers.get('Content-Type')).toContain('text/html');
    expect(markdown.headers.get('Content-Type')).toBe('text/markdown; charset=utf-8');
    expect(await html.text()).toBe('');
    expect(await markdown.text()).toBe('');
  });

  it('converts Markdown asset failures into plain-text 503 responses', async () => {
    const response = await run(
      '/agent-quickstart',
      { headers: { Accept: 'text/markdown' } },
      { throwOnFetch: true }
    );

    expect(response.status).toBe(503);
    expect(response.headers.get('Content-Type')).toBe('text/plain; charset=utf-8');
  });
});
