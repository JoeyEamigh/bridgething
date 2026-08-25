import { beforeAll, describe, expect, test } from 'bun:test';
import { resolve } from 'node:path';
import { createImageShell, type ShellFsManifest } from './shell-fs.ts';

const publicDir = resolve(import.meta.dirname, '..', '..', 'public');
const manifest = (await Bun.file(resolve(publicDir, 'shell-fs.json'))
  .json()
  .catch(() => null)) as ShellFsManifest | null;

let bash: Awaited<ReturnType<typeof createImageShell>>;

describe.skipIf(!manifest)('image shell', () => {
  beforeAll(async () => {
    const blobDir = resolve(publicDir, 'shell-fs');
    bash = await createImageShell(manifest!, hash => Bun.file(resolve(blobDir, `${hash}.txt`)).text());
  });

  test('root listing matches the image layout', async () => {
    const res = await bash.exec('ls /');
    expect(res.exitCode).toBe(0);
    for (const entry of ['bin', 'etc', 'usr', 'var', 'proc']) {
      expect(res.stdout).toContain(entry);
    }
  });

  test('merged usr: /bin resolves through the symlink', async () => {
    const res = await bash.exec('readlink /bin && ls /bin | head -3');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain('usr/bin');
  });

  test('real text content ships', async () => {
    const res = await bash.exec('cat /etc/hostname');
    expect(res.exitCode).toBe(0);
    expect(res.stdout.trim().length).toBeGreaterThan(0);
  });

  test('systemd units are readable and greppable', async () => {
    const res = await bash.exec('grep -l bridgething /usr/lib/systemd/system/*.service | head -3');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain('.service');
  });

  test('elf binaries are placeholders that admit it', async () => {
    const res = await bash.exec('cat /usr/lib/systemd/systemd');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain('ELF aarch64');
    expect(res.stdout).toContain('not part of the web build');
  });

  test('big text lazy-fetches for real: dino.html is the actual game', async () => {
    const res = await bash.exec('head -c 400 /usr/lib/bridgething/webapps/stock/dino.html');
    expect(res.exitCode).toBe(0);
    expect(res.stdout.toLowerCase()).toContain('<!doctype html');
    expect(res.stdout).not.toContain('web build');
  });

  test('non-elf binaries are not labeled elf', async () => {
    const res = await bash.exec("find /usr/share -name '*.png' | head -1");
    const png = res.stdout.trim();
    expect(png.length).toBeGreaterThan(0);
    const out = await bash.exec(`cat '${png}'`);
    expect(out.stdout).toContain('binary,');
    expect(out.stdout).not.toContain('ELF');
  });

  test('stub commands not in the image are hidden from ls', async () => {
    const res = await bash.exec('ls /usr/bin');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).not.toContain('html-to-markdown');
  });

  test('pipes and cwd option work', async () => {
    const res = await bash.exec('ls | wc -l', { cwd: '/etc' });
    expect(res.exitCode).toBe(0);
    expect(Number(res.stdout.trim())).toBeGreaterThan(10);
  });

  test('cd && pwd resolves for the ui cwd tracker', async () => {
    const res = await bash.exec('cd /usr/lib/bridgething && pwd');
    expect(res.exitCode).toBe(0);
    expect(res.stdout.trim()).toBe('/usr/lib/bridgething');
  });

  test('uname reports the device', async () => {
    const res = await bash.exec('uname -a');
    expect(res.stdout).toContain('bridgething');
    expect(res.stdout).toContain('aarch64');
  });

  test('runtime-rendered /etc/superbird resolves and carries the demo identity', async () => {
    const res = await bash.exec('cat /etc/superbird');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain('"serialNumber": "SB2202C0FFEE"');
    expect(res.stdout).toContain('"btMac": "DE:CA:FB:C0:FF:EE"');
  });

  test('bluez alias seeded under the efuse mac', async () => {
    const res = await bash.exec("cat '/var/lib/bluetooth/DE:CA:FB:C0:FF:EE/settings'");
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain('Alias=Car Thing (SN: FFEE)');
  });

  test('per-serial networkd unit lives in /run', async () => {
    const res = await bash.exec('grep Address /run/systemd/network/11-usb-ncm.network');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toMatch(/Address=10\.42\.1\.\d+\/29/);
  });

  test('/opt/bridgething resolves into the bandaid seed', async () => {
    const res = await bash.exec('cat /opt/bridgething/.adopted-image-version && ls /opt/bridgething/daemon');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain('bridgething.current');
  });

  test('daemon state dirs exist on the data partition', async () => {
    const res = await bash.exec('ls /var/lib/bridgething/state');
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain('bridgething.db');
    expect(res.stdout).toContain('assets');
  });
});
