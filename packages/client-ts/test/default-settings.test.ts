import { describe, expect, test } from 'bun:test';
import { Window } from 'happy-dom';

import { DEFAULT_SETTINGS_PAGE } from '../dist/default-settings.js';
import type { ConfigField, SettingsHost } from '../src/settings-protocol.js';
import { settingsPage, type SettingsPage } from '../src/testing.js';
import { THEME_DEFAULTS } from '../src/theme.js';

function frame(): Window {
  return new Window({
    url: 'about:blank',
    settings: { enableJavaScriptEvaluation: true, suppressInsecureJavaScriptEnvironmentWarning: true },
  });
}

async function open(bind: (target: string) => SettingsHost, target = 'weather'): Promise<SettingsPage<string, Window>> {
  const page = settingsPage(frame(), { target, bind });
  page.load(DEFAULT_SETTINGS_PAGE);
  await page.until(() => page.frame.document.querySelector('.bt-field, .bt-note:not(:empty)') !== null);
  return page;
}

function host(done: () => void): SettingsHost {
  const unused = () => Promise.reject(new Error('unused'));
  return {
    context: unused,
    'config.fields': () =>
      Promise.resolve([{ type: 'secret', data: { key: 'token', label: 'API token' } }] as ConfigField[]),
    'config.list': () => Promise.resolve([]),
    'config.set': ({ key, value }) => Promise.resolve({ key, value }),
    'config.delete': ({ key }) => Promise.resolve({ key, value: null }),
    'doc.get': unused,
    'doc.list': unused,
    'doc.set': unused,
    'doc.delete': unused,
    fetch: unused,
    'auth.authorize': unused,
    pick: unused,
    'blob.create': unused,
    'blob.append': unused,
    'blob.put': unused,
    'blob.delete': unused,
    'blob.list': unused,
    done,
  };
}

describe('the default settings page', () => {
  test('is one self-contained html file', () => {
    expect(DEFAULT_SETTINGS_PAGE.startsWith('<!doctype html>')).toBe(true);
    expect(DEFAULT_SETTINGS_PAGE).not.toMatch(/<script[^>]+src=/);
    expect(DEFAULT_SETTINGS_PAGE).not.toMatch(/<link[^>]+stylesheet/);
  });

  test('renders the config form through the bridge and closes on done', async () => {
    let closed = 0;
    const page = await open(() => host(() => (closed += 1)));

    expect(page.text()).toContain('API token');
    expect(page.frame.document.querySelector('#bt-field-token')?.getAttribute('type')).toBe('password');

    page.press('Done');
    await page.until(() => closed > 0);
    expect(closed).toBe(1);

    await page.close();
  });

  test('done writes a field that still has focus before it closes', async () => {
    const events: string[] = [];
    const page = await open(() => ({
      ...host(() => events.push('done')),
      'config.set': ({ key, value }) => {
        events.push(`set ${key}=${value}`);
        return Promise.resolve({ key, value });
      },
    }));

    page.enter('#bt-field-token', 's3cret');
    page.press('Done');
    await page.until(() => events.includes('done'));

    expect(events).toEqual(['set token=s3cret', 'done']);

    await page.close();
  });

  test('done stays open and says why when the device refuses the last edit', async () => {
    let closed = 0;
    const page = await open(() => ({
      ...host(() => (closed += 1)),
      'config.set': () => Promise.reject(new Error('the device said no')),
    }));

    page.enter('#bt-field-token', 's3cret');
    page.press('Done');
    await page.until(() => page.text().includes('the device said no'));

    expect(page.text()).toContain('the device said no');
    expect(closed).toBe(0);

    await page.close();
  });

  test('done closes a page whose form never loaded', async () => {
    let closed = 0;
    const page = await open(() => ({
      ...host(() => (closed += 1)),
      'config.fields': () => Promise.reject(new Error('the car thing went away')),
    }));
    expect(page.text()).toContain('the car thing went away');

    page.press('Done');
    await page.until(() => closed > 0);

    expect(closed).toBe(1);

    await page.close();
  });

  test('switches to another app on a retarget without reloading', async () => {
    const apps: Record<string, ConfigField[]> = {
      weather: [{ type: 'string', data: { key: 'zip', label: 'ZIP code' } }] as ConfigField[],
      browser: [{ type: 'string', data: { key: 'bookmarks', label: 'Bookmarks' } }] as ConfigField[],
    };
    const theme = { scheme: 'light' as const, ...THEME_DEFAULTS, colors: THEME_DEFAULTS.colors.light };
    const context = (target: string) => ({ webappId: target, name: target, version: '1.0.0', deviceId: 'd', theme });
    const page = await open(target => ({
      ...host(() => {}),
      context: () => Promise.resolve(context(target)),
      'config.fields': () => Promise.resolve(apps[target]),
    }));
    const marker = page.frame.document.createElement('i');
    page.frame.document.body.appendChild(marker);
    expect(page.text()).toContain('ZIP code');

    page.session.retarget('browser', context('browser'));
    await page.until(() => page.text().includes('Bookmarks'));

    expect(page.text()).toContain('Bookmarks');
    expect(page.text()).not.toContain('ZIP code');
    expect(marker.isConnected).toBe(true);
    expect(page.frame.document.querySelector('style[data-bridgething-theme]')?.textContent).toContain(
      '--bt-scheme:light;',
    );

    await page.close();
  });

  test('shows an app picked while the page was still loading with no app', async () => {
    const fields = [{ type: 'string', data: { key: 'zip', label: 'ZIP code' } }] as ConfigField[];
    const page = settingsPage<string, Window>(frame(), {
      target: null,
      bind: target => ({
        ...host(() => {}),
        context: () => Promise.resolve({ webappId: target, name: target, version: '1.0.0', deviceId: 'd' }),
        'config.fields': () => Promise.resolve(fields),
      }),
    });
    page.session.retarget('weather', { webappId: 'weather', name: 'weather', version: '1.0.0', deviceId: 'd' });

    page.load(DEFAULT_SETTINGS_PAGE);
    await page.until(() => page.text().includes('ZIP code'));

    expect(page.text()).toContain('ZIP code');

    await page.close();
  });
});
