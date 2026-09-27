import { listedWebapps, type WebappSlotName } from '@bridgething/catalog';
import type {
  BridgethingActiveWebapp,
  BridgethingWebappInfo,
  BridgethingWebappSlots,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
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
  listed: boolean;
};

export type DeviceSlots = {
  slots: BridgethingWebappSlots | null;
  error: string | null;
};

const empty: DeviceWebapps = { list: [], active: null, listed: false };
const emptySlots: DeviceSlots = { slots: null, error: null };

type WebappsState = {
  byDevice: Record<string, DeviceWebapps>;
  slotsByDevice: Record<string, DeviceSlots>;
};

export const useWebappsStore = create<WebappsState>(() => ({
  byDevice: {},
  slotsByDevice: {},
}));

export function registerWebappsDomain(): void {
  registerDomain({
    name: 'webapps',
    apply: event => {
      if (event.type === 'webappsChanged') {
        const { deviceId, webapps, active, listed } = event.entry;
        useWebappsStore.setState(s => ({
          byDevice: {
            ...s.byDevice,
            [deviceId]: { list: webapps, active: active ?? null, listed },
          },
        }));
        void refreshSlots(deviceId);
        return;
      }
      if (event.type === 'peerDisconnected') {
        useWebappsStore.setState(s => {
          const byDevice = { ...s.byDevice };
          const slotsByDevice = { ...s.slotsByDevice };
          delete byDevice[event.peerId];
          delete slotsByDevice[event.peerId];
          return { byDevice, slotsByDevice };
        });
      }
    },
    reconcile: snapshot => {
      useWebappsStore.setState({
        byDevice: Object.fromEntries(
          snapshot.webapps.map(entry => [
            entry.deviceId,
            {
              list: entry.webapps,
              active: entry.active ?? null,
              listed: entry.listed,
            },
          ]),
        ),
      });
      for (const entry of snapshot.webapps) void refreshSlots(entry.deviceId);
    },
  });
}

function putSlots(deviceId: string, entry: DeviceSlots): void {
  useWebappsStore.setState(s => ({
    slotsByDevice: { ...s.slotsByDevice, [deviceId]: entry },
  }));
}

export async function refreshSlots(deviceId: string): Promise<void> {
  try {
    putSlots(deviceId, {
      slots: await getSession().getWebappSlots(deviceId),
      error: null,
    });
  } catch (err) {
    useWebappsStore.setState(s => ({
      slotsByDevice: {
        ...s.slotsByDevice,
        [deviceId]: {
          slots: s.slotsByDevice[deviceId]?.slots ?? null,
          error: describeError(err),
        },
      },
    }));
  }
}

export async function assignSlot(
  deviceId: string,
  slot: WebappSlotName,
  id?: string,
): Promise<void> {
  putSlots(deviceId, {
    slots: await getSession().setWebappSlot(deviceId, slot, id),
    error: null,
  });
}

export function useSlots(deviceId: string | null): DeviceSlots {
  return useWebappsStore(
    useShallow(s =>
      deviceId ? (s.slotsByDevice[deviceId] ?? emptySlots) : emptySlots,
    ),
  );
}

export function heldBy(
  slots: BridgethingWebappSlots | null,
  slot: WebappSlotName,
): string | null {
  if (!slots) return null;
  return (slot === 'launcher' ? slots.launcher : slots.overlay) ?? null;
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
  launcherId: string | null,
): AppTile[] {
  const updatable = new Set(updatableIds.map(id => id.toLowerCase()));
  const active = activeId?.toLowerCase() ?? null;
  const launcher = launcherId?.toLowerCase() ?? null;

  return listedWebapps(list)
    .filter(info => Boolean(info.id))
    .map(info => {
      const key = info.id.toLowerCase();
      const builtin = info.source === 'builtin';
      const state: AppTile['state'] = updatable.has(key)
        ? { label: 'update', tone: 'accent' }
        : key === active
          ? { label: 'active', tone: 'ok' }
          : key === launcher
            ? { label: 'home screen', tone: 'neutral' }
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
