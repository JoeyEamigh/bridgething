export const OAUTH_CALLBACK_PATH = '/oauth/callback';
export const APP_CALLBACK_URL = 'bridgething://oauth/callback';

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function escapeScript(value: string): string {
  return JSON.stringify(value).replace(/</g, '\\u003c').replace(/>/g, '\\u003e').replace(/&/g, '\\u0026');
}

export function appCallbackUrl(url: URL): string {
  return `${APP_CALLBACK_URL}${url.search}`;
}

export function oauthBounceBody(target: string): string {
  const attr = escapeHtml(target);

  return [
    '<!doctype html>',
    '<html lang="en">',
    '<head>',
    '<meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width,initial-scale=1">',
    '<meta name="robots" content="noindex,nofollow">',
    `<meta http-equiv="refresh" content="0;url=${attr}">`,
    '<title>bridgething</title>',
    '<style>body{background:#0a0c0e;color:#efefef;font-family:ui-monospace,SF Mono,Menlo,Consolas,monospace;',
    'display:flex;min-height:100dvh;align-items:center;justify-content:center;margin:0;padding:1.5rem;text-align:center}',
    'a{color:#00a8e8}</style>',
    '</head>',
    '<body>',
    `<p>open bridgething to finish signing in<br><a href="${attr}">${attr}</a></p>`,
    `<script>location.replace(${escapeScript(target)})</script>`,
    '</body>',
    '</html>',
  ].join('');
}

export function oauthBounce(url: URL): Response {
  return new Response(oauthBounceBody(appCallbackUrl(url)), {
    status: 200,
    headers: {
      'content-type': 'text/html; charset=utf-8',
      'cache-control': 'no-store',
      'referrer-policy': 'no-referrer',
    },
  });
}
