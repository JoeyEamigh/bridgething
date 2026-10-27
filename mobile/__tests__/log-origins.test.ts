import { rig, type Rig } from './harness';

type StoreLine = {
  seq: number;
  ts: number;
  origin: string;
  level: string;
  message: string;
};

function line(
  seq: number,
  ts: number,
  origin: string,
  message: string,
): StoreLine {
  return { seq, ts, origin, level: 'info', message };
}

async function streaming(r: Rig): Promise<void> {
  await r.diagnostics.startDiagnostics();
}

function messages(r: Rig): string[] {
  return r.diagnostics
    .mergeLogs(r.diagnostics.useDiagnosticsStore.getState().logs)
    .map(l => l.message);
}

async function settle(): Promise<void> {
  for (let tick = 0; tick < 10; tick++) await Promise.resolve();
}

describe('phone log backfill', () => {
  beforeEach(() => jest.useFakeTimers());
  afterEach(() => jest.useRealTimers());

  it('seeds the current launch from the persisted archive when phone streaming starts', async () => {
    const r = rig();
    await streaming(r);
    r.native.__returns.set('logArchives', [
      { id: '900', startedAt: 900, bytes: 10, pinned: false, current: false },
      { id: '1000', startedAt: 1000, bytes: 20, pinned: false, current: true },
    ]);
    r.native.__returns.set('logArchiveLines', [
      line(0, 1001, 'local', 'phone woke up'),
      line(1, 1002, 'local', 'phone signed in'),
    ]);

    r.diagnostics.useDiagnosticsStore.getState().setLocalLogStreaming(true);
    await settle();

    expect(messages(r)).toEqual(['phone woke up', 'phone signed in']);
    expect(r.native.__calls).toContain('logArchiveLines');
  });

  it('leaves the daemon lines teed into the archive to the device buffer', async () => {
    const r = rig();
    await streaming(r);
    r.native.__returns.set('logArchives', [
      { id: '1000', startedAt: 1000, bytes: 20, pinned: false, current: true },
    ]);
    r.native.__returns.set('logArchiveLines', [
      line(0, 1001, 'local', 'phone woke up'),
      line(1, 1002, 'device', '[daemon] the spool filled'),
    ]);

    r.diagnostics.useDiagnosticsStore.getState().setLocalLogStreaming(true);
    await settle();

    expect(messages(r)).toEqual(['phone woke up']);
  });

  it('never wipes device lines when phone streaming is switched on', async () => {
    const r = rig();
    await streaming(r);
    r.native.__returns.set('deviceLogSnapshot', [
      line(1, 500, 'device', 'daemon booted'),
    ]);
    r.native.__returns.set('logArchives', [
      { id: '1000', startedAt: 1000, bytes: 20, pinned: false, current: true },
    ]);
    r.native.__returns.set('logArchiveLines', [
      line(0, 600, 'local', 'phone woke up'),
    ]);

    r.diagnostics.useDiagnosticsStore.getState().setDeviceLogStreaming(true);
    await settle();
    expect(messages(r)).toEqual(['daemon booted']);

    r.diagnostics.useDiagnosticsStore.getState().setLocalLogStreaming(true);
    await settle();

    expect(messages(r)).toEqual(['daemon booted', 'phone woke up']);
  });

  it('a device re-seed replaces only the device buffer', async () => {
    const r = rig();
    await streaming(r);
    r.native.__returns.set('logArchives', [
      { id: '1000', startedAt: 1000, bytes: 20, pinned: false, current: true },
    ]);
    r.native.__returns.set('logArchiveLines', [
      line(0, 600, 'local', 'phone woke up'),
    ]);
    r.diagnostics.useDiagnosticsStore.getState().setLocalLogStreaming(true);
    await settle();

    r.native.__returns.set('deviceLogSnapshot', [
      line(1, 700, 'device', 'daemon reconnected'),
    ]);
    r.diagnostics.useDiagnosticsStore.getState().setDeviceLogStreaming(true);
    await settle();

    expect(messages(r)).toEqual(['phone woke up', 'daemon reconnected']);
  });

  it('routes live lines to their own origin and merges them in time order', async () => {
    const r = rig();
    await streaming(r);
    r.diagnostics.useDiagnosticsStore.getState().setLocalLogStreaming(true);
    await settle();

    r.emit('log', 'device', 'warn', 'from the car thing', 1000);
    r.emit('log', 'local', 'info', 'from the phone', 2000);
    jest.advanceTimersByTime(200);

    const state = r.diagnostics.useDiagnosticsStore.getState();
    expect(state.logs.device.map(l => l.message)).toEqual([
      'from the car thing',
    ]);
    expect(state.logs.local.map(l => l.message)).toEqual(['from the phone']);
    expect(messages(r)).toEqual(['from the car thing', 'from the phone']);
  });

  it('a replayed burst keeps the times the device logged it, in order', async () => {
    const r = rig();
    await streaming(r);
    r.native.__returns.set('deviceLogSnapshot', []);
    r.diagnostics.useDiagnosticsStore.getState().setDeviceLogStreaming(true);
    await settle();

    r.emit('log', 'device', 'info', 'live after the toggle', 5000);
    r.emit('log', 'device', 'info', 'an hour of history', 1000);
    r.emit('log', 'device', 'info', 'a minute of history', 3000);
    jest.advanceTimersByTime(200);

    const device = r.diagnostics.useDiagnosticsStore.getState().logs.device;
    expect(device.map(l => [l.message, l.ts])).toEqual([
      ['an hour of history', 1000],
      ['a minute of history', 3000],
      ['live after the toggle', 5000],
    ]);
  });

  it('clearing the view empties both origins', async () => {
    const r = rig();
    await streaming(r);
    r.diagnostics.useDiagnosticsStore.getState().setLocalLogStreaming(true);
    await settle();
    r.emit('log', 'device', 'info', 'from the car thing', 1000);
    r.emit('log', 'local', 'info', 'from the phone', 2000);
    jest.advanceTimersByTime(200);
    expect(messages(r)).toHaveLength(2);

    r.diagnostics.useDiagnosticsStore.getState().clearDeviceLogs();

    expect(messages(r)).toEqual([]);
  });
});
