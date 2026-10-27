import type { Theme } from '@bridgething/client/settings-protocol';
import { Button, documentTheme, hasSettings, SectionEmpty, SettingsFrame, Spinner, watchTheme } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';
import { useEffect, useMemo, useRef } from 'preact/hooks';

import { useDesktop } from '../desktop.ts';
import { Icon } from '../lib/icons.tsx';
import { abandonPick } from '../lib/picker.ts';
import { PATHS } from '../routes.ts';
import { webapps } from '../stores/session.ts';
import { desktopSettings, forwardDocChanges, settingsState, settingsTarget } from '../stores/settings-host.ts';
import { ErrorNote } from './Screen.tsx';

export function WebappSettingsFrame({ open }: { open: string | null }): VNode {
  const session = useDesktop();
  const { route } = useLocation();

  const state = settingsState.value;
  const target = settingsTarget.value;
  const installed = webapps.data.value;
  const visible = open !== null;

  const theme = useRef<Theme>(useMemo(documentTheme, []));
  const close = (): void => {
    if (open !== null) route(PATHS.app(open));
  };
  const closer = useRef(close);
  closer.current = close;

  const settings = useMemo(
    () => desktopSettings({ session, theme: () => theme.current, done: () => closer.current() }),
    [session],
  );

  useEffect(() => {
    if (target !== null) settings.select(target);
  }, [settings, target, installed]);

  useEffect(() => () => abandonPick(session), [session, open]);

  useEffect(() => {
    if (!visible) return;
    settings.reveal();
    return () => {
      settings.hide();
      if (document.activeElement instanceof HTMLIFrameElement) document.activeElement.blur();
    };
  }, [settings, visible]);

  useEffect(
    () =>
      watchTheme(next => {
        theme.current = next;
        settings.themeChanged(next);
      }),
    [settings],
  );

  useEffect(() => forwardDocChanges(settings), [settings]);

  const shown = open ? (installed.find(app => app.id === open) ?? null) : null;
  const ready = open !== null && open === state.selected && settings.ready(state);

  return (
    <div class={visible ? 'route-enter flex h-full min-h-0 min-w-0 flex-1 flex-col' : 'hidden'}>
      <header class="flex shrink-0 items-center gap-3 border-b border-rule bg-screen px-6 py-3">
        <Button variant="ghost" size="sm" icon={<Icon name="back" size={14} />} onClick={close}>
          {shown?.name ?? 'all apps'}
        </Button>
        <span class="truncate font-mono text-eyebrow tracking-[0.18em] text-muted uppercase">settings</span>
      </header>

      {ready ? null : open !== null && !webapps.pending.value && (!shown || !hasSettings(shown)) ? (
        <div class="px-6 py-5">
          <SectionEmpty>{shown ? 'no settings' : 'this app is not installed on the connected Car Thing'}</SectionEmpty>
        </div>
      ) : state.failure && open === state.selected ? (
        <div class="flex flex-col items-start gap-3 px-6 py-5">
          <ErrorNote>{state.failure}</ErrorNote>
          <Button size="sm" onClick={settings.retry}>
            retry
          </Button>
        </div>
      ) : (
        <div class="flex min-h-0 flex-1 items-center justify-center">
          <Spinner />
        </div>
      )}

      <SettingsFrame
        settings={settings}
        loaded={state.loaded}
        title={shown ? `${shown.name} settings` : 'settings'}
        class={ready ? 'mx-auto min-h-0 w-full max-w-4xl flex-1 border-0 bg-bg' : 'hidden'}
      />
    </div>
  );
}
