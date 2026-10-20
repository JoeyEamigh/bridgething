import type { BridgethingWebappInfo } from '@bridgething/session-react-native';

import { configFields, hasSettings } from '../lib/webapps';

function webapp(
  overrides: Partial<BridgethingWebappInfo>,
): BridgethingWebappInfo {
  return {
    id: '019e7f1a-bcea-7187-a438-4e182be86a33',
    name: 'weather',
    source: 'installed',
    role: 'standard',
    version: '1.0.0',
    configJson: '[]',
    permissions: [],
    ...overrides,
  };
}

const ZIP = '[{"type":"string","data":{"key":"zip","label":"ZIP code"}}]';

describe('webapp settings gate', () => {
  test('reads config fields in the wire shape', () => {
    expect(configFields(webapp({ configJson: ZIP }))).toEqual([
      { type: 'string', data: { key: 'zip', label: 'ZIP code' } },
    ]);
  });

  test('opens settings for config fields alone', () => {
    expect(hasSettings(webapp({ configJson: ZIP }))).toBe(true);
  });

  test('opens settings for a settings page alone', () => {
    expect(hasSettings(webapp({ settingsHash: 'abc' }))).toBe(true);
  });

  test('has nothing to open without either', () => {
    expect(hasSettings(webapp({}))).toBe(false);
  });
});
