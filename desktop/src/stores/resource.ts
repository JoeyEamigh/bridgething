import { describeError } from '@bridgething/ui';
import { batch, effect, signal, type Signal } from '@preact/signals';

export type Store<T> = {
  data: Signal<T>;
  error: Signal<string | undefined>;
  pending: Signal<boolean>;
  settled: Signal<boolean>;
  refresh: () => Promise<void>;
};

export type Keyed<T> = {
  at: (key: string, pull: () => Promise<T>) => Store<T>;
  refreshAll: () => void;
};

export function resource<T>(initial: T, pull: () => Promise<T>): Store<T> {
  const data = signal(initial);
  const error = signal<string | undefined>(undefined);
  const pending = signal(false);
  const settled = signal(false);

  let inflight: Promise<void> | null = null;
  let again = false;

  const once = async (): Promise<void> => {
    pending.value = true;
    try {
      const pulled = await pull();
      batch(() => {
        data.value = pulled;
        error.value = undefined;
        settled.value = true;
      });
    } catch (reason) {
      batch(() => {
        data.value = initial;
        error.value = describeError(reason);
      });
    } finally {
      pending.value = false;
    }
  };

  const refresh = (): Promise<void> => {
    if (inflight) {
      again = true;
      return inflight;
    }
    inflight = (async () => {
      do {
        again = false;
        await once();
      } while (again);
      inflight = null;
    })();
    return inflight;
  };

  return { data, error, pending, settled, refresh };
}

export function settled<T>(store: Store<T>): Promise<T> {
  return new Promise(resolve => {
    const stop = effect(() => {
      if (store.pending.value) return;
      resolve(store.data.value);
      queueMicrotask(() => stop());
    });
  });
}

export function keyed<T>(initial: T): Keyed<T> {
  const held = new Map<string, Store<T>>();

  return {
    at(key, pull) {
      const found = held.get(key);
      if (found) return found;

      const made = resource(initial, pull);
      made.pending.value = true;
      held.set(key, made);
      queueMicrotask(() => void made.refresh());
      return made;
    },
    refreshAll() {
      for (const store of held.values()) void store.refresh();
    },
  };
}
