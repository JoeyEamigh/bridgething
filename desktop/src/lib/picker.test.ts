import { PICK_BUSY } from '@bridgething/client/settings-protocol';
import type { PickedMedia } from '@bridgething/companion-types';
import { afterEach, describe, expect, mock, test } from 'bun:test';

type Gate<T> = { promise: Promise<T>; open(value: T): void; fail(err: Error): void };

function gate<T>(): Gate<T> {
  let open!: (value: T) => void;
  let fail!: (err: Error) => void;
  const promise = new Promise<T>((resolve, reject) => {
    open = resolve;
    fail = reject;
  });
  return { promise, open, fail };
}

let dialogs: Array<Gate<string | null>> = [];

mock.module('@tauri-apps/plugin-dialog', () => ({
  open: () => {
    const shown = gate<string | null>();
    dialogs.push(shown);
    return shown.promise;
  },
  save: async () => null,
}));

const { abandonPick, pickMedia } = await import('./picker.ts');

const PICKED: PickedMedia = {
  handle: 'h1',
  mime: 'video/mp4',
  size: 3,
  width: 854,
  height: 480,
  durationMs: 4000,
  preview: null,
};

function imports() {
  const imported: Array<Gate<PickedMedia>> = [];
  const cancelled: number[] = [];
  let next = 0;
  return {
    imported,
    cancelled,
    session: {
      mediaImportTicket: async () => (next += 1),
      importPickedMedia: (_ticket: number) => {
        const running = gate<PickedMedia>();
        imported.push(running);
        return running.promise;
      },
      cancelMediaImport: async (ticket: number) => void cancelled.push(ticket),
    },
  };
}

const tick = (): Promise<void> => new Promise(resolve => setTimeout(resolve, 0));

describe('the desktop media pick', () => {
  afterEach(() => {
    for (const shown of dialogs) shown.open(null);
    dialogs = [];
  });

  test('a second pick while one is open is refused with busy and the first still lands', async () => {
    const device = imports();
    const first = pickMedia(device.session, 'video', null);
    await tick();

    await expect(pickMedia(device.session, 'image', null)).rejects.toThrow(PICK_BUSY);
    expect(dialogs).toHaveLength(1);

    dialogs[0]!.open('/tmp/clip.mov');
    await tick();
    device.imported[0]!.open(PICKED);
    expect(await first).toEqual(PICKED);

    const again = pickMedia(device.session, 'image', null);
    await tick();
    expect(dialogs).toHaveLength(2);
    dialogs[1]!.open(null);
    expect(await again).toBeNull();
  });

  test('abandoning a pick mid import cancels the import and frees the picker', async () => {
    const device = imports();
    const first = pickMedia(device.session, 'video', null);
    await tick();
    dialogs[0]!.open('/tmp/clip.mov');
    await tick();
    expect(device.imported).toHaveLength(1);

    abandonPick(device.session);
    expect(device.cancelled).toEqual([1]);
    device.imported[0]!.fail(new Error('failed: cancelled'));
    expect(await first).toBeNull();

    const next = pickMedia(device.session, 'image', null);
    await tick();
    expect(dialogs).toHaveLength(2);
    dialogs[1]!.open(null);
    expect(await next).toBeNull();
  });

  test('a pick abandoned while its dialog is open never starts an import', async () => {
    const device = imports();
    const first = pickMedia(device.session, 'video', null);
    await tick();

    abandonPick(device.session);
    dialogs[0]!.open('/tmp/clip.mov');

    expect(await first).toBeNull();
    expect(device.imported).toHaveLength(0);
    expect(device.cancelled).toEqual([]);
  });

  test('an abandoned pick holds the picker until its dialog actually closes', async () => {
    const device = imports();
    const first = pickMedia(device.session, 'video', null);
    await tick();

    abandonPick(device.session);
    await expect(pickMedia(device.session, 'image', null)).rejects.toThrow(PICK_BUSY);
    expect(dialogs).toHaveLength(1);

    dialogs[0]!.open(null);
    expect(await first).toBeNull();

    const next = pickMedia(device.session, 'image', null);
    await tick();
    expect(dialogs).toHaveLength(2);
    dialogs[1]!.open(null);
    expect(await next).toBeNull();
  });

  test('abandoning the same pick twice cancels its import once', async () => {
    const device = imports();
    const first = pickMedia(device.session, 'video', null);
    await tick();
    dialogs[0]!.open('/tmp/clip.mov');
    await tick();

    abandonPick(device.session);
    abandonPick(device.session);
    expect(device.cancelled).toEqual([1]);
    device.imported[0]!.fail(new Error('failed: cancelled'));
    expect(await first).toBeNull();
  });

  test('a finished import the page has walked away from is dropped', async () => {
    const device = imports();
    const first = pickMedia(device.session, 'video', null);
    await tick();
    dialogs[0]!.open('/tmp/clip.mov');
    await tick();

    abandonPick(device.session);
    device.imported[0]!.open(PICKED);

    expect(await first).toBeNull();
  });

  test('abandoning with nothing in flight does nothing', () => {
    const device = imports();
    abandonPick(device.session);
    expect(device.cancelled).toEqual([]);
  });
});
