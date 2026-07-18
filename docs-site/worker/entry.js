const MARKDOWN_CONTENT_TYPE = 'text/markdown; charset=utf-8';
const TEXT_CONTENT_TYPE = 'text/plain; charset=utf-8';
const LEGACY_GUIDE_PATHS = new Map([
  ['/tutorials/edge-contracts-cloud', '/guides/edge-contracts-cloud/'],
  ['/tutorials/edge-contracts-cloud/', '/guides/edge-contracts-cloud/'],
  ['/tutorials/edge-contracts-cloud.md', '/guides/edge-contracts-cloud.md'],
  ['/en/tutorials/edge-contracts-cloud', '/en/guides/edge-contracts-cloud/'],
  ['/en/tutorials/edge-contracts-cloud/', '/en/guides/edge-contracts-cloud/'],
  ['/en/tutorials/edge-contracts-cloud.md', '/en/guides/edge-contracts-cloud.md'],
]);

function successHeaders(sourceHeaders, contentType) {
  const headers = new Headers(sourceHeaders);
  if (contentType) headers.set('Content-Type', contentType);
  headers.set('X-Content-Type-Options', 'nosniff');
  headers.set('Vary', 'Accept');
  return headers;
}

function plainResponse(message, status, requestMethod, extraHeaders) {
  const headers = successHeaders(extraHeaders, TEXT_CONTENT_TYPE);
  headers.set('Cache-Control', 'no-store');
  return new Response(requestMethod === 'HEAD' ? null : message, { status, headers });
}

function markdownAssetPath(pathname) {
  if (pathname === '/') return '/index.md';
  if (pathname.endsWith('.md')) return pathname;
  const trimmedPath = pathname.endsWith('/') ? pathname.slice(0, -1) : pathname;
  const lastSegment = trimmedPath.slice(trimmedPath.lastIndexOf('/') + 1);
  if (lastSegment.includes('.')) return null;
  return `${trimmedPath}.md`;
}

function legacyGuideRedirect(url) {
  const pathname = LEGACY_GUIDE_PATHS.get(url.pathname);
  if (!pathname) return null;

  const redirectUrl = new URL(url);
  redirectUrl.pathname = pathname;
  return Response.redirect(redirectUrl, 308);
}

async function fetchAsset(request, env, assetPath) {
  const sourceUrl = new URL(request.url);
  const assetUrl = assetPath ? new URL(assetPath, sourceUrl) : sourceUrl;
  return env.ASSETS.fetch(new Request(assetUrl, request));
}

export default {
  async fetch(request, env) {
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      return plainResponse('Method not allowed.\n', 405, request.method, {
        Allow: 'GET, HEAD',
      });
    }

    const url = new URL(request.url);
    const redirect = legacyGuideRedirect(url);
    if (redirect) return redirect;

    const accept = request.headers.get('Accept') || '';
    const wantsMarkdown = url.pathname.endsWith('.md') || accept.includes('text/markdown');

    if (wantsMarkdown) {
      const assetPath = markdownAssetPath(url.pathname);
      if (assetPath === null) {
        return plainResponse('Document not found. See /llms.txt for the index.\n', 404, request.method);
      }

      let response;
      try {
        response = await fetchAsset(request, env, assetPath);
      } catch {
        return plainResponse('Documentation temporarily unavailable.\n', 503, request.method);
      }
      if (!response.ok) {
        return plainResponse('Document not found. See /llms.txt for the index.\n', 404, request.method);
      }

      return new Response(request.method === 'HEAD' ? null : response.body, {
        status: response.status,
        headers: successHeaders(response.headers, MARKDOWN_CONTENT_TYPE),
      });
    }

    let response;
    try {
      response = await fetchAsset(request, env);
    } catch {
      return plainResponse('Documentation temporarily unavailable.\n', 503, request.method);
    }
    const contentType = url.pathname.endsWith('.txt') ? TEXT_CONTENT_TYPE : undefined;
    return new Response(request.method === 'HEAD' ? null : response.body, {
      status: response.status,
      statusText: response.statusText,
      headers: successHeaders(response.headers, contentType),
    });
  },
};
