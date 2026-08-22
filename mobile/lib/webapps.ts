import { listedWebapps } from '@bridgething/catalog';
import type {
  BridgethingActiveWebapp,
  BridgethingWebappInfo,
} from '@bridgething/session-react-native';
import {
  errorCodes,
  isErrorWithCode,
  keepLocalCopy,
  pick,
  types,
} from '@react-native-documents/picker';
import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';

import { getSession, registerDomain } from './bridge';
import type { Tone } from './theme';

export type DeviceWebapps = {
  list: BridgethingWebappInfo[];
  active: BridgethingActiveWebapp | null;
};

const empty: DeviceWebapps = { list: [], active: null };

type WebappsState = {
  byDevice: Record<string, DeviceWebapps>;
};

export const useWebappsStore = create<WebappsState>(() => ({ byDevice: {} }));

export function registerWebappsDomain(): void {
  registerDomain({
    name: 'webapps',
    apply: event => {
      if (event.type === 'webappsChanged') {
        const { deviceId, webapps, active } = event.entry;
        useWebappsStore.setState(s => ({
          byDevice: {
            ...s.byDevice,
            [deviceId]: { list: webapps, active: active ?? null },
          },
        }));
        return;
      }
      if (event.type === 'peerDisconnected') {
        useWebappsStore.setState(s => {
          const next = { ...s.byDevice };
          delete next[event.peerId];
          return { byDevice: next };
        });
      }
    },
    reconcile: snapshot =>
      useWebappsStore.setState({
        byDevice: Object.fromEntries(
          snapshot.webapps.map(entry => [
            entry.deviceId,
            { list: entry.webapps, active: entry.active ?? null },
          ]),
        ),
      }),
  });
}

export function useWebapps(deviceId: string | null): DeviceWebapps {
  return useWebappsStore(
    useShallow(s => (deviceId ? (s.byDevice[deviceId] ?? empty) : empty)),
  );
}

export type AppTile = {
  id: string;
  name: string;
  iconHash?: string;
  builtin: boolean;
  state: { label: string; tone: Tone } | null;
};

export function appTiles(
  list: BridgethingWebappInfo[],
  activeId: string | null,
  updatableIds: string[],
): AppTile[] {
  const updatable = new Set(updatableIds.map(id => id.toLowerCase()));
  const active = activeId?.toLowerCase() ?? null;

  return listedWebapps(list)
    .filter(info => Boolean(info.id))
    .map(info => {
      const key = info.id.toLowerCase();
      const builtin = info.source === 'builtin';
      const state: AppTile['state'] = updatable.has(key)
        ? { label: 'update', tone: 'accent' }
        : key === active
          ? { label: 'active', tone: 'ok' }
          : builtin
            ? { label: 'built-in', tone: 'neutral' }
            : null;
      return {
        id: info.id,
        name: info.name,
        iconHash: info.iconHash,
        builtin,
        state,
      };
    });
}

export function installedWebapps(
  deviceId: string | null,
): BridgethingWebappInfo[] {
  if (!deviceId) return [];
  return useWebappsStore.getState().byDevice[deviceId]?.list ?? [];
}

export async function installPickedWebapp(
  deviceId: string,
): Promise<BridgethingWebappInfo | null> {
  const archive = await pickWebappArchive();
  if (!archive) return null;
  return getSession().installWebappFromUri(deviceId, archive);
}

async function pickWebappArchive(): Promise<string | null> {
  let picked;
  try {
    [picked] = await pick({ type: [types.zip], mode: 'import' });
  } catch (err) {
    if (isErrorWithCode(err) && err.code === errorCodes.OPERATION_CANCELED) {
      return null;
    }
    throw err;
  }

  const fileName = picked.name ?? 'webapp.zip';
  const [copy] = await keepLocalCopy({
    files: [{ uri: picked.uri, fileName }],
    destination: 'cachesDirectory',
  });
  if (copy.status === 'error') {
    throw new Error(`could not read ${fileName}: ${copy.copyError}`);
  }
  return copy.localUri;
}
