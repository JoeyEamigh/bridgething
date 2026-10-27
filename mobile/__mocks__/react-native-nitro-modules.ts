import type {
  BridgethingDeviceMeta,
  BridgethingSessionPeer,
  BridgethingSessionSnapshot,
} from '@bridgething/session-react-native';

type Callback = (...args: unknown[]) => void;

export function emptySnapshot(): BridgethingSessionSnapshot {
  return {
    hostInfo: {
      appName: 'bridgething',
      appVersion: '0.6.0',
      osName: 'iOS',
      osVersion: '26.0',
      hostIdentifier: 'host',
      libVersion: '0.6.0',
      libbridgethingVersion: '0.6.0',
      adapterVersion: '0.6.0',
    },
    providers: [],
    providerPriority: [],
    peers: [],
    ancsAuthStatuses: [],
    deviceMeta: [],
    capabilityFlags: {
      geo: true,
      notifications: true,
      netFetch: true,
      netWs: true,
      audioTts: true,
      voiceModel: true,
    },
    voiceModel: { status: 'absent', receivedBytes: 0, totalBytes: 0 },
    webapps: [],
    otaRuns: [],
    otaAvailable: [],
    otaPoll: {},
    otaQueues: [],
  };
}

export type FakeNative = Record<string, unknown> & {
  __handlers: Map<string, Callback>;
  __returns: Map<string, unknown>;
  __calls: string[];
  __world: BridgethingSessionSnapshot;
  __emit(event: string, ...args: unknown[]): void;
};

function fold(
  world: BridgethingSessionSnapshot,
  event: string,
  args: unknown[],
): void {
  switch (event) {
    case 'peerConnected':
    case 'peerLinkFailed': {
      const peer = args[0] as BridgethingSessionPeer;
      world.peers = [...world.peers.filter(p => p.id !== peer.id), peer];
      return;
    }
    case 'peerDisconnected': {
      const peerId = args[0] as string;
      world.peers = world.peers.filter(p => p.id !== peerId);
      world.deviceMeta = world.deviceMeta.filter(e => e.deviceId !== peerId);
      return;
    }
    case 'deviceMetaChanged': {
      const deviceId = args[0] as string;
      world.deviceMeta = [
        ...world.deviceMeta.filter(e => e.deviceId !== deviceId),
        { deviceId, meta: args[1] as BridgethingDeviceMeta },
      ];
      return;
    }
    case 'resumed':
      Object.assign(world, args[0] as BridgethingSessionSnapshot);
  }
}

const natives = new Map<string, FakeNative>();

const syncMethods = new Set([
  'abandonPick',
  'otaRunProgress',
  'setLogStreamingEnabled',
  'setLocalLogStreamingEnabled',
]);

function handlerKey(method: string): string {
  const rest = method.slice('setOn'.length);
  return rest.charAt(0).toLowerCase() + rest.slice(1);
}

function makeNative(name: string): FakeNative {
  const handlers = new Map<string, Callback>();
  const returns = new Map<string, unknown>();
  const calls: string[] = [];
  const world = emptySnapshot();
  let catalogSources = ['https://apps.bridgething.com/catalog.json'];
  if (name === 'BridgethingSession') {
    returns.set('snapshot', () => Promise.resolve({ ...world }));
    returns.set('catalogSources', () => Promise.resolve([...catalogSources]));
    returns.set('setCatalogSources', (urls: unknown) => {
      catalogSources = [...new Set(urls as string[])];
      return Promise.resolve([...catalogSources]);
    });
  }
  const target = {
    __handlers: handlers,
    __returns: returns,
    __calls: calls,
    __world: world,
    __emit(event: string, ...args: unknown[]) {
      fold(world, event, args);
      const handler = handlers.get(event);
      if (!handler)
        throw new Error(`no native handler registered for "${event}"`);
      handler(...args);
    },
  } as FakeNative;

  return new Proxy(target, {
    get(obj, prop: string) {
      if (prop in obj) return obj[prop as keyof FakeNative];
      if (prop === 'then') return undefined;
      if (prop.startsWith('setOn')) {
        return (cb: Callback) => handlers.set(handlerKey(prop), cb);
      }
      return (...args: unknown[]) => {
        calls.push(prop);
        const canned = returns.get(prop);
        const value =
          typeof canned === 'function'
            ? (canned as (...a: unknown[]) => unknown)(...args)
            : canned;
        if (syncMethods.has(prop)) return value ?? null;
        return typeof canned === 'function' ? value : Promise.resolve(value);
      };
    },
  }) as FakeNative;
}

export const NitroModules = {
  createHybridObject<T>(name: string): T {
    let native = natives.get(name);
    if (!native) {
      native = makeNative(name);
      natives.set(name, native);
    }
    return native as unknown as T;
  },
};

export function fakeNative(name = 'BridgethingSession'): FakeNative {
  return NitroModules.createHybridObject<FakeNative>(name);
}

export function resetNatives(): void {
  natives.clear();
}
