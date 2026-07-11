/**
 * Serves the Starlight static build. If a request either targets a `.md`
 * URL directly, or asks for markdown via `Accept`, it is rewritten to the
 * matching page's `.md` sibling (emitted by scripts/emit-markdown-pages.mjs)
 * before falling through to normal static-asset serving.
 */
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const accept = request.headers.get('Accept') || '';

    if (!url.pathname.endsWith('.md') && accept.includes('text/markdown')) {
      let path = url.pathname;
      if (path.endsWith('/')) path = path.slice(0, -1);
      if (path === '') path = '/index';

      const mdUrl = new URL(path + '.md', url);
      const mdResponse = await env.ASSETS.fetch(new Request(mdUrl, request));
      if (mdResponse.ok) {
        return new Response(mdResponse.body, {
          status: mdResponse.status,
          headers: { 'Content-Type': 'text/markdown; charset=utf-8' },
        });
      }
    }

    return env.ASSETS.fetch(request);
  },
};
