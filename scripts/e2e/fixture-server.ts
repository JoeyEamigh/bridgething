#!/usr/bin/env bun

const port = Number(process.env.E2E_FIXTURE_PORT ?? '8899');
const version = process.env.E2E_COMPANION_VERSION ?? '99.0.0';

const companion = {
  android: {
    version,
    url: `http://10.0.2.2:${port}/companion/android/${version}/bridgething-${version}.apk`,
    size: 1,
    sha256: '0'.repeat(64),
    released_at: '2026-01-01T00:00:00Z',
  },
};

Bun.serve({
  port,
  hostname: '0.0.0.0',
  fetch(request) {
    const { pathname } = new URL(request.url);
    console.log(`[e2e-fixtures] ${request.method} ${pathname}`);
    if (pathname === '/companion.json') return Response.json(companion);
    return new Response('not found', { status: 404 });
  },
});

console.log(`[e2e-fixtures] serving companion.json v${version} on :${port}`);
