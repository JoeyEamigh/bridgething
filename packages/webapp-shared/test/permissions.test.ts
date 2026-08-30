import { describe, expect, test } from 'bun:test';
import { denoFlags, PermissionError } from '../src/permissions.ts';

describe('denoFlags', () => {
  test('all collapses the whole argv', () => {
    expect(denoFlags(['all', 'net:example.com'])).toEqual(['--allow-all']);
  });

  test('scopes of one kind fold into one flag in declaration order', () => {
    expect(denoFlags(['read:/var/a', 'net:a.example', 'read:/tmp'])).toEqual([
      '--allow-net=a.example',
      '--allow-read=/var/a,/tmp',
    ]);
  });

  test('a bare descriptor beats its scoped siblings', () => {
    expect(denoFlags(['run:ffmpeg', 'run'])).toEqual(['--allow-run']);
  });

  test('an empty declaration grants nothing', () => {
    expect(denoFlags([])).toEqual([]);
  });

  test('a host:port scope keeps its own colon', () => {
    expect(denoFlags(['net:example.com:443'])).toEqual(['--allow-net=example.com:443']);
  });

  test('tilde expands only for path-shaped kinds', () => {
    expect(denoFlags(['read:~/Music', 'write:~', 'env:~', 'sys:hostname'], '/home/me')).toEqual([
      '--allow-read=/home/me/Music',
      '--allow-write=/home/me',
      '--allow-env=~',
      '--allow-sys=hostname',
    ]);
  });

  test('malformed descriptors are rejected the way the daemon rejects them', () => {
    for (const descriptor of ['', 'network', 'all:everything', 'net:', 'READ', 'net:a.example,b.example', 'read:,']) {
      expect(() => denoFlags([descriptor])).toThrow(PermissionError);
    }
  });
});
