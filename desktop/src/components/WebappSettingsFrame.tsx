import type { WebappInfo } from '@bridgething/companion-types';
import { Button, describeError, Spinner } from '@bridgething/ui';
import { useSignalEffect } from '@preact/signals';
import type { VNode } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { useDesktop } from '../desktop.ts';
import { toWireConfigField } from '../lib/config-field.ts';
import { Icon } from '../lib/icons.tsx';
import { peers, webappDocFor } from '../stores/session.ts';
import { ErrorNote } from './Screen.tsx';

type BridgeRequest = { id: number; verb: string; payload?: Record<string, unknown> };

export function WebappSettingsFrame({ webapp, onClose }: { webapp: WebappInfo; onClose: () => void }): VNode {
  const session = useDesktop();
  const frame = useRef<HTMLIFrameElement>(null);
  const [page, setPage] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const docs = webappDocFor(webapp.id);
  const known = useRef<Map<string, string> | null>(null);

  const deliver = (payload: unknown): void => {
    frame.current?.contentWindow?.postMessage(JSON.stringify(payload), '*');
  };

  useEffect(() => {
    let held: string | null = null;
    let live = true;
    known.current = null;
    setPage(null);
    setFailure(null);

    void (async () => {
      try {
        const resource = await session.webappResource(webapp.id, 'settings');
        if (!live) return;
        held = URL.createObjectURL(new Blob([Uint8Array.from(resource.bytes)], { type: resource.mime ?? 'text/html' }));
        setPage(held);
      } catch (reason) {
        if (live) setFailure(describeError(reason));
      }
    })();

    return () => {
      live = false;
      if (held) URL.revokeObjectURL(held);
    };
  }, [session, webapp.id, webapp.settingsHash]);

  useEffect(() => {
    const deviceId = peers.value.find(peer => peer.status === 'connected')?.id ?? '';

    const answer = async (verb: string, payload: Record<string, unknown>): Promise<unknown> => {
      const key = typeof payload.key === 'string' ? payload.key : '';
      const value = typeof payload.value === 'string' ? payload.value : '';
      switch (verb) {
        case 'context':
          return { webappId: webapp.id, name: webapp.name, version: webapp.version, deviceId };
        case 'config.fields':
          return webapp.config.map(toWireConfigField);
        case 'config.list':
          return session.webappConfig(webapp.id);
        case 'config.set':
          await session.setWebappConfigField(webapp.id, key, value);
          return { key, value };
        case 'config.delete': {
          await session.deleteWebappConfigField(webapp.id, key);
          const fresh = await session.webappConfig(webapp.id);
          return { key, value: fresh.find(entry => entry.key === key)?.value ?? null };
        }
        case 'doc.get':
          return { key, value: await session.webappDocEntry(webapp.id, key) };
        case 'doc.list':
          return session.webappDoc(webapp.id);
        case 'doc.set':
          await session.setWebappDoc(webapp.id, key, value);
          return { key, value };
        case 'doc.delete':
          await session.deleteWebappDoc(webapp.id, key);
          return { key, value: null };
        default:
          throw new Error(`unknown settings bridge verb: ${verb}`);
      }
    };

    const onMessage = (event: MessageEvent): void => {
      if (!frame.current || event.source !== frame.current.contentWindow) return;
      if (typeof event.data !== 'string') return;

      let request: BridgeRequest;
      try {
        request = JSON.parse(event.data) as BridgeRequest;
      } catch {
        return;
      }
      if (typeof request.id !== 'number' || typeof request.verb !== 'string') return;
      if (request.verb === 'done') {
        onClose();
        return;
      }

      void (async () => {
        try {
          deliver({ id: request.id, ok: true, value: await answer(request.verb, request.payload ?? {}) });
        } catch (reason) {
          deliver({ id: request.id, ok: false, error: describeError(reason) });
        }
      })();
    };

    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [session, webapp, onClose]);

  useSignalEffect(() => {
    const next = new Map(docs.data.value.map(entry => [entry.key, entry.value]));
    const previous = known.current;
    known.current = next;
    if (!previous) return;

    for (const [key, value] of next) if (previous.get(key) !== value) deliver({ event: 'docChanged', key, value });
    for (const key of previous.keys()) if (!next.has(key)) deliver({ event: 'docChanged', key, value: null });
  });

  return (
    <div class="flex h-full min-h-0 min-w-0 flex-1 flex-col">
      <header class="flex shrink-0 items-center gap-3 border-b border-rule bg-screen px-6 py-3">
        <Button variant="ghost" size="sm" icon={<Icon name="back" size={14} />} onClick={onClose}>
          {webapp.name}
        </Button>
        <span class="truncate font-mono text-eyebrow tracking-[0.18em] text-muted uppercase">settings</span>
      </header>

      {failure ? (
        <div class="px-6 py-5">
          <ErrorNote>{failure}</ErrorNote>
        </div>
      ) : !page ? (
        <div class="flex min-h-0 flex-1 items-center justify-center">
          <Spinner />
        </div>
      ) : (
        <iframe
          ref={frame}
          src={page}
          title={`${webapp.name} settings`}
          sandbox="allow-scripts allow-forms"
          class="min-h-0 w-full flex-1 border-0 bg-bg"
        />
      )}
    </div>
  );
}
