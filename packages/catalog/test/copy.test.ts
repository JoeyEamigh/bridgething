import { describe, expect, test } from 'bun:test';
import { permissionLabel } from '../src/index.ts';

describe('permissionLabel', () => {
  test('names every permission the daemon grants', () => {
    expect(permissionLabel('net.fetch')).toBe('internet');
    expect(permissionLabel('net.ws')).toBe('websockets');
    expect(permissionLabel('net.proxy')).toBe('TCP tunnel');
    expect(permissionLabel('geo')).toBe('location');
    expect(permissionLabel('notifications')).toBe('notifications');
    expect(permissionLabel('audio')).toBe('sound');
    expect(permissionLabel('audio.tts')).toBe('sound');
    expect(permissionLabel('mic')).toBe('microphone');
  });

  test('an unknown permission is shown as written rather than dropped', () => {
    expect(permissionLabel('camera')).toBe('camera');
  });
});
