import { asJson, defineExtension, json, type ExtensionContext } from '@bridgething/extension';
import { memoryFromMeminfo, memoryFromVmStat, type Memory, type PhysicalMemory } from '../src/memory.ts';
import type { Stats } from '../src/stats.ts';

const INTERVAL_MS = 2_000;

async function darwinMemory(total: number): Promise<PhysicalMemory | null> {
  try {
    const out = await new Deno.Command('vm_stat', { stdout: 'piped', stderr: 'null' }).output();
    return out.success ? memoryFromVmStat(new TextDecoder().decode(out.stdout), total) : null;
  } catch {
    return null;
  }
}

async function linuxMemory(): Promise<PhysicalMemory | null> {
  try {
    return memoryFromMeminfo(await Deno.readTextFile('/proc/meminfo'));
  } catch {
    return null;
  }
}

async function memory(): Promise<Memory> {
  const info = Deno.systemMemoryInfo();
  const exact =
    Deno.build.os === 'darwin'
      ? await darwinMemory(info.total)
      : Deno.build.os === 'linux'
        ? await linuxMemory()
        : null;
  return {
    ...(exact ?? { total: info.total, used: info.total - info.available, cached: null }),
    swapTotal: info.swapTotal,
    swapFree: info.swapFree,
  };
}

async function snapshot(): Promise<Stats> {
  const [one = 0, five = 0, fifteen = 0] = Deno.loadavg();
  return {
    type: 'stats',
    host: { name: Deno.hostname(), os: Deno.build.os, arch: Deno.build.arch, release: Deno.osRelease() },
    cores: navigator.hardwareConcurrency,
    load: [one, five, fifteen],
    memory: await memory(),
    uptimeSeconds: Deno.osUptime(),
    addresses: Deno.networkInterfaces()
      .filter(nic => nic.family === 'IPv4' && !nic.address.startsWith('127.'))
      .map(nic => ({ name: nic.name, address: nic.address })),
    at: Date.now(),
  };
}

function push(deliver: (stats: Stats) => void): void {
  void snapshot().then(deliver);
}

let timer: ReturnType<typeof setInterval> | undefined;

function reconcile(ctx: ExtensionContext): void {
  const watched = ctx.devices.some(device => device.active);
  if (watched && timer === undefined) {
    timer = setInterval(() => push(stats => ctx.broadcast(json(stats))), INTERVAL_MS);
  } else if (!watched && timer !== undefined) {
    clearInterval(timer);
    timer = undefined;
  }
}

defineExtension({
  start(ctx) {
    ctx.on('device', event => {
      if (event.type !== 'disconnected' && event.device.active) push(stats => event.device.send(json(stats)));
      reconcile(ctx);
    });

    ctx.on('message', (device, message) => {
      if (asJson<{ type?: string }>(message)?.type === 'refresh') push(stats => device.send(json(stats)));
    });
  },
  stop() {
    clearInterval(timer);
  },
});
