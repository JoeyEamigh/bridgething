import { rig } from './harness';

describe('recording a javascript crash', () => {
  test('a fatal survives the launch that died', () => {
    const r = rig();
    const boom = new Error('cannot read properties of undefined');
    boom.stack = 'Error: cannot read properties of undefined\n    at OtaCard';

    r.crash.recordCrash(boom, 'handler', true);

    const next = r.relaunch();
    const held = next.crash.useCrashStore.getState().last;
    expect(held?.message).toBe('cannot read properties of undefined');
    expect(held?.stack).toContain('at OtaCard');
    expect(held?.fatal).toBe(true);
    expect(held?.origin).toBe('handler');
  });

  test('a boundary catch carries the component stack', () => {
    const r = rig();

    r.crash.recordCrash(
      new Error('render blew up'),
      'boundary',
      false,
      '\n    in OtaRunProgress',
    );

    const held = r.crash.useCrashStore.getState().last;
    expect(held?.fatal).toBe(false);
    expect(held?.componentStack).toContain('in OtaRunProgress');
  });

  test('a rejection that is not an error still describes itself', () => {
    const r = rig();

    r.crash.recordCrash({ kind: 'NotConnected' }, 'handler', true);

    expect(r.crash.useCrashStore.getState().last?.message).toBe(
      'not connected',
    );
  });

  test('clearing drops it for good', () => {
    const r = rig();
    r.crash.recordCrash(new Error('gone'), 'handler', true);

    r.crash.clearLastCrash();

    expect(r.crash.useCrashStore.getState().last).toBeNull();
    expect(r.relaunch().crash.useCrashStore.getState().last).toBeNull();
  });

  test('the global handler records and still lets react native fatal', () => {
    const r = rig();
    const downstream = jest.fn();
    ErrorUtils.setGlobalHandler(downstream);

    r.crash.installCrashHandlers();
    const boom = new Error('unhandled');
    ErrorUtils.getGlobalHandler()(boom, true);

    expect(r.crash.useCrashStore.getState().last?.message).toBe('unhandled');
    expect(downstream).toHaveBeenCalledWith(boom, true);
  });

  test('a stack too big for the store is clipped, not dropped', () => {
    const r = rig();
    const boom = new Error('deep');
    boom.stack = 'x'.repeat(20_000);

    r.crash.recordCrash(boom, 'handler', true);

    const held = r.crash.useCrashStore.getState().last;
    expect(held?.stack).toHaveLength(8_000);
  });
});
