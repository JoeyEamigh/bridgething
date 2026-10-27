import type { AppVersion, CatalogAppListing } from '@bridgething/catalog';
import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';

import { installApp } from '../../lib/catalog';
import { useDeviceBusy, useOtaProgress, useQueuedInstall } from '../../lib/ota';

export function useStoreInstall(
  deviceId: string | null,
  listing: CatalogAppListing,
) {
  const appId = listing.app.id;
  const progress = useOtaProgress(deviceId, 'webapp');
  const installing =
    progress && !progress.run.outcome && progress.run.webappId === appId
      ? progress
      : null;
  const busyElsewhere = useDeviceBusy(deviceId) && !installing;
  const waiting = useQueuedInstall(deviceId, appId);
  const [failed, setFailed] = useState<string | null>(null);

  const install = async (version: AppVersion | null) => {
    if (!deviceId || !version) return;
    setFailed(null);
    try {
      await installApp(deviceId, listing, version);
    } catch (err) {
      setFailed(describeError(err));
    }
  };

  return {
    installing,
    waiting,
    queued: waiting != null && busyElsewhere,
    failed,
    install,
  };
}
