import type { InstalledWebapp, WebappDocChange } from '@bridgething/browser';
import { useQuery, useSession, type DeviceSession, type Query, type Topic } from '@bridgething/ui';

export interface BrowserBackend extends DeviceSession {
  readonly host: 'browser';

  installWebappBytes(bundle: Uint8Array, provenance?: string): Promise<InstalledWebapp>;
  watchDocs(watcher: (change: WebappDocChange) => void): () => void;
}

export function isBrowser(session: DeviceSession): session is BrowserBackend {
  return (session as Partial<BrowserBackend>).host === 'browser';
}

export function useBrowser(): BrowserBackend {
  const session = useSession();
  if (!isBrowser(session)) throw new Error('the device console is mounted over a backend that is not the browser one');
  return session;
}

export function useBrowserQuery<T>(
  topics: Topic[],
  pull: (session: BrowserBackend) => Promise<T>,
  deps: unknown[] = [],
): Query<T> {
  const session = useBrowser();
  return useQuery(topics, () => pull(session), deps);
}
