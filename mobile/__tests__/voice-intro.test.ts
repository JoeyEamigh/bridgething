import { peer, voiceTurn } from './fixtures';
import { rig } from './harness';
import type { ConditionId } from '../lib/status';

describe('the onboarding hey-bridgething gate', () => {
  test('a wake word turn opening on the device reads as listening on the phone', () => {
    const r = rig();
    r.emit('peerConnected', peer());

    r.emit('voiceTurnChanged', voiceTurn());

    expect(
      r.session.voiceIntroState(
        r.session.useSessionStore.getState().lastVoiceTurn,
      ),
    ).toBe('listening');
  });

  test('a wake word turn that resolves to a real intent passes the gate', () => {
    const r = rig();
    r.emit('peerConnected', peer());

    r.emit('voiceTurnChanged', voiceTurn());
    r.emit(
      'voiceTurnChanged',
      voiceTurn({ phase: 'resolved', transcript: 'next song', intent: 'NEXT' }),
    );

    const held = r.session.useSessionStore.getState().lastVoiceTurn;
    expect(r.session.voiceIntroState(held)).toBe('heard');
    expect(held?.transcript).toBe('next song');
  });

  test('a turn the model declined is a miss, so the gate keeps teaching', () => {
    const r = rig();

    r.emit(
      'voiceTurnChanged',
      voiceTurn({ phase: 'resolved', transcript: 'mm', intent: 'NO_INTENT' }),
    );

    expect(
      r.session.voiceIntroState(
        r.session.useSessionStore.getState().lastVoiceTurn,
      ),
    ).toBe('missed');
  });

  test('a turn that only reached clarify has not proven the pipeline either', () => {
    const r = rig();

    r.emit(
      'voiceTurnChanged',
      voiceTurn({ phase: 'resolved', transcript: 'play', intent: 'CLARIFY' }),
    );

    expect(
      r.session.voiceIntroState(
        r.session.useSessionStore.getState().lastVoiceTurn,
      ),
    ).toBe('missed');
  });

  test('a cancelled turn settles rather than leaving the gate listening forever', () => {
    const r = rig();

    r.emit('voiceTurnChanged', voiceTurn());
    r.emit('voiceTurnChanged', voiceTurn({ phase: 'cancelled' }));

    expect(
      r.session.voiceIntroState(
        r.session.useSessionStore.getState().lastVoiceTurn,
      ),
    ).toBe('missed');
  });

  test('a push-to-talk turn never satisfies a lesson about the wake word', () => {
    const r = rig();

    r.emit(
      'voiceTurnChanged',
      voiceTurn({
        trigger: 'pushToTalk',
        phase: 'resolved',
        transcript: 'next song',
        intent: 'NEXT',
      }),
    );

    expect(
      r.session.voiceIntroState(
        r.session.useSessionStore.getState().lastVoiceTurn,
      ),
    ).toBe('waiting');
  });

  test('with no turn seen yet the gate is waiting, not missed', () => {
    const r = rig();

    expect(
      r.session.voiceIntroState(
        r.session.useSessionStore.getState().lastVoiceTurn,
      ),
    ).toBe('waiting');
  });

  test('skipping is remembered across a relaunch so onboarding does not re-ask', () => {
    const r = rig();
    r.storage.setVoiceIntroOutcome('skipped');

    expect(r.relaunch().storage.getVoiceIntroOutcome()).toBe('skipped');
  });

  test('a phone that never ran the intro has no outcome recorded', () => {
    expect(rig().storage.getVoiceIntroOutcome()).toBeNull();
  });

  test('a paired phone that has not settled the intro is shown the step', () => {
    expect(rig().setup.setupSteps(true)).toEqual([
      'signIn',
      'pair',
      'voice',
      'permissions',
    ]);
  });

  test('an unpaired phone has no device to teach the wake word on', () => {
    expect(rig().setup.setupSteps(false)).not.toContain('voice');
  });

  test.each(['heard', 'skipped'] as const)(
    'onboarding relaunched after the intro was %s drops the step entirely',
    outcome => {
      const r = rig();
      expect(r.setup.setupSteps(true)).toContain('voice');

      r.storage.setVoiceIntroOutcome(outcome);

      expect(r.relaunch().setup.setupSteps(true)).toEqual([
        'signIn',
        'pair',
        'permissions',
      ]);
    },
  );
});

describe('what onboarding leaves behind', () => {
  const SKIPPED: ConditionId[] = ['notSignedIn', 'noDevice'];

  test('every step a user can skip hands back the page that finishes it', () => {
    const steps = rig().setup.setupSteps(true);

    expect(SKIPPED.map(id => rig().setup.stepForCondition(steps, id))).toEqual([
      0, 1,
    ]);
  });

  test('the pair page keeps its index when the voice lesson is not in the run', () => {
    const r = rig();

    expect(
      r.setup.stepForCondition(r.setup.setupSteps(false), 'noDevice'),
    ).toBe(1);
  });

  test('a condition onboarding never asked about has no page to return to', () => {
    const r = rig();
    const steps = r.setup.setupSteps(true);

    for (const id of ['offline', 'updateFailed', 'storeUnavailable'] as const) {
      expect(r.setup.stepForCondition(steps, id)).toBeNull();
    }
  });

  test('the finish page states what is left rather than dropping it silently', () => {
    const r = rig();

    expect(r.setup.pendingSummary(2)).toBe('2 left');
    expect(r.setup.pendingSummary(1)).toBe('1 left');
    expect(r.setup.pendingSummary(0)).toBe('all set');
  });
});
