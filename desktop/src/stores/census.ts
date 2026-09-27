import { reportInstalled } from '@bridgething/catalog';
import { effect } from '@preact/signals';

import { selectedMeta, webapps } from './session.ts';

export function watchInstallCensus(): () => void {
  return effect(() => {
    const listing = webapps.data.value;
    const settled = webapps.settled.value;
    const failed = webapps.error.value !== undefined;
    const serial = selectedMeta.value?.serialNumber ?? null;
    if (!settled || failed || !serial) return;

    reportInstalled({
      deviceId: serial,
      apps: listing
        .filter(app => app.source === 'installed' && app.provenance)
        .map(app => ({ appId: app.id, sourceUrl: app.provenance!, version: app.version })),
    });
  });
}
