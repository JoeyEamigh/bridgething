import { permissionLabel } from '@bridgething/catalog';

import { humanizePermission } from '../lib/webapp-permissions';

describe('humanizePermission', () => {
  it('titles a permission with the label every surface shares', () => {
    for (const perm of ['net.fetch', 'net.ws', 'geo', 'notifications', 'mic'])
      expect(humanizePermission(perm).title).toBe(permissionLabel(perm));
  });

  it('carries no explanatory chin', () => {
    expect(Object.keys(humanizePermission('net.fetch'))).toEqual([
      'icon',
      'title',
    ]);
  });

  it('falls back to the raw permission string', () => {
    expect(humanizePermission('something.new')).toEqual({
      icon: 'Shield',
      title: 'something.new',
    });
  });

  it('treats audio and audio.tts as the same capability', () => {
    expect(humanizePermission('audio')).toEqual(
      humanizePermission('audio.tts'),
    );
  });
});
