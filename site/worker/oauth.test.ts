import { describe, expect, test } from 'bun:test';
import { appCallbackUrl, oauthBounce, oauthBounceBody } from './oauth.ts';

function bounce(href: string): Response {
  return oauthBounce(new URL(href));
}

function hrefIn(body: string): string {
  return /<a href="([^"]*)"/.exec(body)?.[1] ?? '';
}

function refreshIn(body: string): string {
  return /content="0;url=([^"]*)"/.exec(body)?.[1] ?? '';
}

describe('oauth callback bounce', () => {
  test('answers 200 so the page the browser renders is the fallback, not a scheme error', async () => {
    const response = bounce('https://bridgething.com/oauth/callback?code=abc&state=xyz');

    expect(response.status).toBe(200);
    expect(response.headers.get('location')).toBeNull();
    expect(response.headers.get('content-type')).toContain('text/html');
  });

  test('the hop into the app happens client-side, by script and by meta refresh', async () => {
    const body = await bounce('https://bridgething.com/oauth/callback?code=abc&state=xyz').text();

    expect(body).toContain(
      '<script>location.replace("bridgething://oauth/callback?code=abc\\u0026state=xyz")</script>',
    );
    expect(refreshIn(body)).toBe('bridgething://oauth/callback?code=abc&amp;state=xyz');
  });

  test('the deep link is visible and clickable for a browser that swallows both', async () => {
    const body = await bounce('https://bridgething.com/oauth/callback?code=abc').text();

    expect(body).toContain('open bridgething to finish signing in');
    expect(hrefIn(body)).toBe('bridgething://oauth/callback?code=abc');
  });

  test('bounces with no query at all rather than a bare question mark', () => {
    expect(appCallbackUrl(new URL('https://bridgething.com/oauth/callback'))).toBe('bridgething://oauth/callback');
  });

  test('a query the url parser already encoded stays encoded in the markup', async () => {
    const body = await bounce(
      'https://bridgething.com/oauth/callback?code=%22%3E%3Cscript%3Ealert(1)%3C/script%3E',
    ).text();

    expect(body).toContain('%3Cscript%3E');
    expect(body.split('</script>')).toHaveLength(2);
  });

  test('a target carrying quotes cannot break out of the href or the refresh', () => {
    const body = oauthBounceBody('bridgething://oauth/callback?code="><img src=x onerror=alert(1)>');

    expect(hrefIn(body)).toBe('bridgething://oauth/callback?code=&quot;&gt;&lt;img src=x onerror=alert(1)&gt;');
    expect(refreshIn(body)).toBe('bridgething://oauth/callback?code=&quot;&gt;&lt;img src=x onerror=alert(1)&gt;');
    expect(body).not.toContain('<img');
  });

  test('a target carrying a closing script tag cannot end the script early', () => {
    const body = oauthBounceBody('bridgething://oauth/callback?code=</script><script>alert(1)</script>');

    expect(body.split('</script>')).toHaveLength(2);
    expect(body).not.toContain('<script>alert(1)');
    expect(body).toContain('\\u003c/script\\u003e\\u003cscript\\u003ealert(1)\\u003c/script\\u003e');
  });

  test('nothing about the callback is cached', () => {
    expect(bounce('https://bridgething.com/oauth/callback?code=abc').headers.get('cache-control')).toBe('no-store');
  });
});
