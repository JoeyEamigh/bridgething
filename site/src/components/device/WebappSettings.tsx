import type { Theme } from '@bridgething/client/settings-protocol';
import type { WebappInfo } from '@bridgething/companion-types';
import {
  Button,
  SectionHeader,
  SettingsFrame,
  Spinner,
  documentTheme,
  warmSettingsState,
  watchTheme,
  type WarmSettingsState,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import { message } from '../../lib/browser-session';
import { useBrowser, useBrowserQuery } from '../../lib/browser-tier';
import { fetchCatalog } from '../../lib/catalog-source';
import { DEFAULT_PAGE, browserSettings } from '../../lib/settings-host';
import { ErrorNote, Section } from './Screen';

export function WebappSettings({ webapp, onClose }: { webapp: WebappInfo; onClose: () => void }): VNode {
  const session = useBrowser();
  const peers = useBrowserQuery(['peers'], s => s.peers());
  const [state, setState] = useState<WarmSettingsState<string, Blob>>(() => warmSettingsState(DEFAULT_PAGE));
  const held = useRef(state);
  const theme = useRef<Theme | null>(null);

  const app = useRef(webapp);
  app.current = webapp;
  const deviceId = useRef('');
  deviceId.current = peers.data?.[0]?.id ?? '';
  const close = useRef(onClose);
  close.current = onClose;

  const settings = useMemo(
    () =>
      browserSettings({
        session,
        store: {
          get: () => held.current,
          set: patch => {
            held.current = { ...held.current, ...patch };
            setState(held.current);
          },
        },
        webapp: () => app.current,
        deviceId: () => deviceId.current,
        theme: () => (theme.current ??= documentTheme()),
        done: () => close.current(),
        explain: message,
        fetchCatalog: source => fetchCatalog(source, { viaRelay: true }),
      }),
    [session],
  );

  useEffect(() => settings.select(webapp.id), [settings, webapp]);

  useEffect(() => {
    settings.reveal();
    const unwatchDocs = session.watchDocs(change => settings.docChanged(change.id, change.key, change.value));
    const unwatchTheme = watchTheme(next => {
      theme.current = next;
      settings.themeChanged(next);
    });
    return () => {
      settings.hide();
      unwatchDocs();
      unwatchTheme();
    };
  }, [session, settings]);

  const ready = settings.ready(state);

  return (
    <Section>
      <SectionHeader
        title={`${webapp.name} settings`}
        hint="changes save to the device as you make them"
        action="done"
        onAction={onClose}
      />
      {ready ? null : state.failure ? (
        <div class="flex flex-col items-start gap-3">
          <ErrorNote>{state.failure}</ErrorNote>
          <Button size="sm" onClick={settings.retry}>
            retry
          </Button>
        </div>
      ) : (
        <div class="border-rule bg-screen flex h-48 items-center justify-center border">
          <Spinner />
        </div>
      )}
      {state.loaded.key === state.wanted ? (
        <SettingsFrame
          settings={settings}
          loaded={state.loaded}
          title={`${webapp.name} settings`}
          class={ready ? 'border-rule bg-bg block h-[70vh] min-h-96 w-full border' : 'hidden'}
        />
      ) : null}
    </Section>
  );
}
