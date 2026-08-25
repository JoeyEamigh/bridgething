import type {
  BluetoothPin,
  BridgethingClient,
  Notification,
  PeerSnapshotMap,
  PhoneCall,
  PhoneCallStatus,
  VoiceActivity,
  VolumeChanged,
} from '@bridgething/client';

import { Overlay, type OverlayConfig } from '../src/index';
import { mountRoot } from '../src/ui';

type Handler<T> = (value: T) => void;

class Topic<T> {
  private readonly subs = new Set<Handler<T>>();
  on = (handler: Handler<T>): (() => void) => {
    this.subs.add(handler);
    return () => this.subs.delete(handler);
  };
  emit(value: T): void {
    for (const handler of this.subs) handler(value);
  }
}

const peerSnapshot = new Topic<PeerSnapshotMap>();
const callStarted = new Topic<PhoneCall>();
const callUpdated = new Topic<PhoneCall>();
const callEnded = new Topic<PhoneCall>();
const posted = new Topic<Notification>();
const updated = new Topic<Notification>();
const removed = new Topic<{ id: string }>();
const pin = new Topic<BluetoothPin>();
const pairingResult = new Topic<unknown>();
const volumeChanged = new Topic<VolumeChanged>();
const voiceActivity = new Topic<VoiceActivity>();

const fakeClient = {
  peer: { onSnapshot: peerSnapshot.on },
  phone: { onCallStarted: callStarted.on, onCallUpdated: callUpdated.on, onCallEnded: callEnded.on },
  notifications: { onPosted: posted.on, onUpdated: updated.on, onRemoved: removed.on },
  bluetooth: { onPin: pin.on, onPairingResult: pairingResult.on },
  audio: { onVolumeChanged: volumeChanged.on },
  voice: {
    onActivity: voiceActivity.on,
    stateGet: async () => ({ ok: false, kind: 'protocol', error: { type: 'unsupported' } }),
  },
} as unknown as BridgethingClient;

const cfg: OverlayConfig = {
  origin: location.origin,
  surfaces: { notifications: true, call: true, pairing: true, connection: true, volume: true, voice: true },
};

const stage = document.getElementById('stage');
if (!stage) throw new Error('missing #stage');
new Overlay(cfg, mountRoot(stage), fakeClient);

function call(status: PhoneCallStatus): PhoneCall {
  return {
    callId: 'demo-call-1',
    remoteId: '+15550123',
    displayName: 'Sam Rivera',
    status,
    direction: 'incoming',
    startedAtUnixS: null,
    label: null,
    addressBookId: null,
    service: 'telephony',
    isConferenced: null,
    conferenceGroup: null,
  };
}

let notificationSeq = 0;
function notification(
  app: string,
  bundleId: string,
  title: string,
  message: string | null,
  silent = false,
): Notification {
  return {
    id: `demo-toast-${notificationSeq++}`,
    app: { bundleId, displayName: app, iconAssetId: null },
    category: 'social',
    title,
    subtitle: null,
    message,
    timestampUnixS: null,
    flags: { silent, important: false },
    positiveAction: null,
    negativeAction: null,
  };
}

function peers(useful: boolean): PeerSnapshotMap {
  return {
    'aa:bb:cc:dd:ee:ff': {
      device: { name: 'iPhone', type: 'iOS', id: 'aa:bb:cc:dd:ee:ff', kind: 'bluetooth', default: true },
      paired: true,
      iap2: useful ? 'identified' : 'none',
      companion: { type: 'none' },
      displayName: 'iPhone',
      language: null,
      uuid: null,
    },
  } as unknown as PeerSnapshotMap;
}

function voice(partial: Partial<VoiceActivity> & Pick<VoiceActivity, 'phase'>): VoiceActivity {
  return {
    streamId: '0199a1f0-0000-7000-8000-000000000001',
    reason: null,
    score: null,
    transcript: null,
    intent: null,
    slots: {},
    stage: null,
    target: null,
    error: null,
    ...partial,
  } as VoiceActivity;
}

let volume = 0.55;
let muted = false;
function emitVolume(): void {
  volumeChanged.emit({ level: volume, muted });
}

const actions: Record<string, () => void> = {
  'call-ring': () => callStarted.emit(call('ringing')),
  'call-active': () => callUpdated.emit(call('active')),
  'call-held': () => callUpdated.emit(call('held')),
  'call-end': () => callEnded.emit(call('disconnected')),
  'toast-msg': () =>
    posted.emit(notification('Messages', 'com.apple.MobileSMS', 'Sam Rivera', 'leaving now, see you in 10')),
  'toast-discord': () =>
    posted.emit(
      notification('Discord', 'com.hammerandchisel.discord', '#superbird', 'new OTA is up on the dev channel'),
    ),
  'toast-long': () =>
    posted.emit(
      notification(
        'Calendar',
        'com.apple.mobilecal',
        'Standup in 5 minutes',
        'this event repeats every weekday and this body is deliberately long enough to exercise the two-line clamp on the toast message.',
      ),
    ),
  'toast-silent': () => posted.emit(notification('Mail', 'com.apple.mobilemail', 'silent', 'must never render', true)),
  'pin-show': () => pin.emit({ pin: '482913', name: 'Bridgething', mac: '30:E3:D6:03:96:1E' }),
  'pin-result': () => pairingResult.emit({}),
  'vol-up': () => {
    muted = false;
    volume = Math.min(1, volume + 0.05);
    emitVolume();
  },
  'vol-down': () => {
    muted = false;
    volume = Math.max(0, volume - 0.05);
    emitVolume();
  },
  'vol-mute': () => {
    muted = !muted;
    emitVolume();
  },
  'voice-listen': () => voiceActivity.emit(voice({ phase: 'listening', reason: 'wakeWord', score: 0.94 })),
  'voice-think': () => voiceActivity.emit(voice({ phase: 'thinking', transcript: 'skip this song' })),
  'voice-done': () =>
    voiceActivity.emit(
      voice({ phase: 'done', transcript: 'skip this song', intent: 'NEXT', stage: 'fastPath', target: 'playback' }),
    ),
  'voice-fail': () =>
    voiceActivity.emit(
      voice({
        phase: 'failed',
        transcript: 'play the new mitski album',
        intent: 'NO_INTENT',
        stage: 'noModel',
        error: { code: 'notDispatchable', msg: 'NO_INTENT is resolved at the companion edge' },
      }),
    ),
  'conn-drop': () => peerSnapshot.emit(peers(false)),
  'conn-back': () => peerSnapshot.emit(peers(true)),
};

for (const button of document.querySelectorAll<HTMLButtonElement>('button[data-act]')) {
  button.addEventListener('click', () => actions[button.dataset.act ?? '']?.());
}
