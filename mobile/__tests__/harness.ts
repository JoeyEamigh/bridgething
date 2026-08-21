import type { FakeNative } from '../__mocks__/react-native-nitro-modules';

type Modules = {
  session: typeof import('../lib/session');
  setup: typeof import('../lib/setup');
  storage: typeof import('../lib/storage');
  webapps: typeof import('../lib/webapps');
  ota: typeof import('../lib/ota');
  companionUpdate: typeof import('../lib/companion-update');
  bridge: typeof import('../lib/bridge');
  catalog: typeof import('../lib/catalog');
  diagnostics: typeof import('../lib/diagnostics');
  permissions: typeof import('react-native-permissions');
  picker: typeof import('../__mocks__/@react-native-documents/picker');
};

export type Rig = Modules & {
  native: FakeNative;
  emit(event: string, ...args: unknown[]): void;
  relaunch(): Rig;
};

const MMKV_KEYS = [
  'setup.completed',
  'setup.voiceIntro',
  'device.ledger',
  'catalog.sources',
  'companionUpdate.dismissed',
];

export type RigOptions = { platform?: 'ios' | 'android' };

function build(opts: RigOptions, carry?: Record<string, string>): Rig {
  jest.resetModules();

  const rn = require('react-native') as typeof import('react-native');
  Object.defineProperty(rn.Platform, 'OS', {
    value: opts.platform ?? 'ios',
    configurable: true,
  });

  const nitro =
    require('../__mocks__/react-native-nitro-modules') as typeof import('../__mocks__/react-native-nitro-modules');
  nitro.resetNatives();

  const storage = require('../lib/storage') as Modules['storage'];
  if (carry)
    for (const [key, value] of Object.entries(carry))
      storage.storage.set(key, value);

  const setup = require('../lib/setup') as Modules['setup'];
  const bridge = require('../lib/bridge') as Modules['bridge'];
  const session = require('../lib/session') as Modules['session'];
  const webapps = require('../lib/webapps') as Modules['webapps'];
  const ota = require('../lib/ota') as Modules['ota'];
  const companionUpdate =
    require('../lib/companion-update') as Modules['companionUpdate'];
  const catalog = require('../lib/catalog') as Modules['catalog'];
  const diagnostics = require('../lib/diagnostics') as Modules['diagnostics'];
  const permissions =
    require('react-native-permissions') as Modules['permissions'];
  const picker = require('@react-native-documents/picker') as Modules['picker'];

  session.registerSessionDomain();
  webapps.registerWebappsDomain();
  ota.registerOtaDomain();
  companionUpdate.registerCompanionUpdateDomain();
  bridge.startBridge();

  const native = nitro.fakeNative();

  return {
    session,
    setup,
    storage,
    webapps,
    ota,
    companionUpdate,
    bridge,
    catalog,
    diagnostics,
    permissions,
    picker,
    native,
    emit(event, ...args) {
      native.__emit(event, ...args);
    },
    relaunch() {
      const bytes: Record<string, string> = {};
      for (const key of MMKV_KEYS) {
        const value = storage.storage.getString(key);
        if (value !== undefined) bytes[key] = value;
      }
      return build(opts, bytes);
    },
  };
}

export function rig(opts: RigOptions = {}): Rig {
  return build(opts);
}
