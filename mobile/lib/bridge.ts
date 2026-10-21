import {
  BridgethingSession,
  type BridgethingSessionSnapshot,
  type SessionEvent,
} from '@bridgething/session-react-native';

let sessionSingleton: BridgethingSession | null = null;

export function getSession(): BridgethingSession {
  if (!sessionSingleton) sessionSingleton = new BridgethingSession();
  return sessionSingleton;
}

export type SessionDomain = {
  name: string;
  apply(event: SessionEvent): void;
  reconcile(snapshot: BridgethingSessionSnapshot): void;
};

const domains: SessionDomain[] = [];
let wired = false;

export function registerDomain(domain: SessionDomain): void {
  if (domains.some(d => d.name === domain.name)) return;
  domains.push(domain);
}

function fanOut(what: string, run: (domain: SessionDomain) => void): void {
  for (const domain of domains) {
    try {
      run(domain);
    } catch (err) {
      console.warn(`[bridgething] ${domain.name} failed to ${what}`, err);
    }
  }
}

const inFlight = new Set<SessionEvent[]>();

function deliver(event: SessionEvent): void {
  if (event.type === 'resumed') {
    fanOut('reconcile', domain => domain.reconcile(event.snapshot));
    return;
  }
  fanOut(`apply ${event.type}`, domain => domain.apply(event));
}

export function startBridge(): void {
  if (wired) return;
  wired = true;
  getSession().subscribe(event => {
    for (const arrived of inFlight) arrived.push(event);
    deliver(event);
  });
}

export async function reconcileAll(): Promise<void> {
  const arrived: SessionEvent[] = [];
  inFlight.add(arrived);
  try {
    const snapshot = await getSession().snapshot();
    fanOut('reconcile', domain => domain.reconcile(snapshot));
    for (const event of arrived) deliver(event);
  } finally {
    inFlight.delete(arrived);
  }
}
