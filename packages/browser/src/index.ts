import type { BridgeThingMeta, LauncherGesture, WebappInfo } from '@bridgething/lib';
import { BRIDGETHING_DEFAULT_HOST, BRIDGETHING_NETWORK_GATEWAY_PORT } from '@bridgething/lib';
import type { DeviceNicknameReply, WebappActive, WebappSlot, WebappSlots } from '@bridgething/lib/gateway';

import type {
  ArtifactUrls,
  CompositeVersion,
  InstalledWebapp,
  OtaDiscoverManifest,
  OtaUpdateKind,
  Phase,
  UpdateEvent,
} from '../wasm/core-wasm.js';
import initWasm, {
  ByteLink,
  DeliverySession,
  artifactUrls,
  discoverManifest,
  parseCompositeVersion,
} from '../wasm/core-wasm.js';

import { pumpSerialPort, requestSerialPort, type SerialPort } from './web-serial.js';

export type {
  ArtifactDigest,
  ArtifactUrls,
  CompositeVersion,
  InstalledWebapp,
  OtaDiscoverManifest,
  OtaManifestChannel,
  OtaManifestRelease,
  OtaPatchDigest,
  OtaReleaseArtifacts,
  OtaUpdateKind,
  Phase,
  PhaseKind,
  PlanStep,
  UpdateEvent,
} from '../wasm/core-wasm.js';
export { permittedSerialPorts, requestSerialPort, serialAvailable } from './web-serial.js';
export type { SerialPort } from './web-serial.js';

export const DEFAULT_HOST = BRIDGETHING_DEFAULT_HOST;

let started: Promise<unknown> | null = null;

export function ready(): Promise<unknown> {
  started ??= initWasm();
  return started;
}

export function gatewayUrl(host: string): string {
  const trimmed = host
    .trim()
    .replace(/^wss?:\/\//, '')
    .replace(/\/+$/, '');
  return `ws://${trimmed.includes(':') ? trimmed : `${trimmed}:${BRIDGETHING_NETWORK_GATEWAY_PORT}`}/`;
}

export async function fetchManifest(rootUrl: string): Promise<OtaDiscoverManifest> {
  await ready();
  return discoverManifest(rootUrl);
}

export async function compositeVersion(raw: string): Promise<CompositeVersion | null> {
  await ready();
  return parseCompositeVersion(raw);
}

export async function otaArtifactUrls(opts: {
  rootUrl: string;
  channel: string;
  daemonVersion: string;
  imageVersion: string;
  imageVariant: string;
}): Promise<ArtifactUrls> {
  await ready();
  return artifactUrls(opts.rootUrl, opts.channel, opts.daemonVersion, opts.imageVersion, opts.imageVariant);
}

export class Device {
  private constructor(
    private readonly session: DeliverySession,
    private readonly detach: () => Promise<void>,
  ) {}

  static async overNetwork(host: string = DEFAULT_HOST): Promise<Device> {
    await ready();
    const session = await DeliverySession.connect(gatewayUrl(host), undefined);
    return new Device(session, async () => {});
  }

  static async overSerial(port?: SerialPort): Promise<Device | null> {
    const opened = port ?? (await requestSerialPort());
    if (!opened) return null;
    await ready();

    let write: (chunk: Uint8Array) => Promise<void> = () => Promise.resolve();
    const link = new ByteLink(chunk => write(chunk));
    const pump = pumpSerialPort(
      opened,
      chunk => link.push(chunk),
      () => link.close(),
    );
    write = chunk => pump.write(chunk);

    const session = await DeliverySession.attach(link, undefined);
    return new Device(session, () => pump.close());
  }

  get id(): string {
    return this.session.deviceId;
  }

  meta(): Promise<BridgeThingMeta | null> {
    return this.session.meta() as Promise<BridgeThingMeta | null>;
  }

  webapps(): Promise<WebappInfo[]> {
    return this.session.webapps() as Promise<WebappInfo[]>;
  }

  activeWebapp(): Promise<WebappActive> {
    return this.session.activeWebapp() as Promise<WebappActive>;
  }

  webappSlots(): Promise<WebappSlots> {
    return this.session.webappSlots() as Promise<WebappSlots>;
  }

  switchWebapp(id: string): Promise<WebappActive> {
    return this.session.switchWebapp(id) as Promise<WebappActive>;
  }

  uninstallWebapp(id: string): Promise<WebappActive> {
    return this.session.uninstallWebapp(id) as Promise<WebappActive>;
  }

  setWebappSlot(slot: WebappSlot, id: string | null): Promise<WebappSlots> {
    return this.session.setWebappSlot(slot, id ?? undefined) as Promise<WebappSlots>;
  }

  setNickname(nickname: string): Promise<DeviceNicknameReply> {
    return this.session.setNickname(nickname) as Promise<DeviceNicknameReply>;
  }

  setLauncherGesture(gesture: LauncherGesture): Promise<void> {
    return this.session.setLauncherGesture(gesture);
  }

  installWebapp(bundle: Uint8Array, provenance?: string): Promise<InstalledWebapp> {
    return this.session.installWebapp(bundle, provenance);
  }

  push(kind: Exclude<OtaUpdateKind, 'image'>, artifact: Uint8Array, label?: string): Promise<Phase> {
    return this.session.push(kind, artifact, label);
  }

  pushImage(swu: Uint8Array, zcks: Map<string, Uint8Array>, updateUrlBase?: string): Promise<Phase> {
    return this.session.pushImage(swu, zcks, updateUrlBase);
  }

  nextEvent(): Promise<UpdateEvent> {
    return this.session.nextEvent();
  }

  closed(): Promise<void> {
    return this.session.closed();
  }

  close(): Promise<void> {
    return this.detach();
  }
}
