import type { SessionPeer } from '@bridgething/companion-types';
import { Button, Wordmark, cx } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';

import { Icon } from '../lib/icons.tsx';
import { SECTIONS, sectionFor } from '../routes.ts';
import { applySelfUpdate, selfUpdate } from '../stores/self-update.ts';

export function Sidebar({ peers }: { peers: SessionPeer[] }): VNode {
  const { path, route } = useLocation();
  const current = sectionFor(path);

  const linked = peers.filter(peer => peer.status === 'connected');
  const failed = peers.filter(peer => peer.status === 'linkFailed');

  return (
    <nav class="flex w-52 shrink-0 flex-col border-r border-rule bg-screen">
      <div class="border-b border-rule px-5 py-5">
        <Wordmark size="sm" />
      </div>

      <ul class="m-0 flex list-none flex-col p-0">
        {SECTIONS.map(section => (
          <li key={section.path}>
            <button
              type="button"
              aria-current={section.path === current ? 'page' : undefined}
              class={cx(
                'flex w-full items-center gap-3 border-l-2 px-5 py-2.5 text-left font-mono text-body lowercase transition-colors duration-150 focus-visible:outline-2 focus-visible:outline-accent focus-visible:-outline-offset-2',
                section.path === current
                  ? 'border-accent bg-accent-soft text-accent'
                  : 'border-transparent text-soft hover:bg-neutral-soft hover:text-off-white active:bg-rule',
              )}
              onClick={() => route(section.path)}>
              <Icon name={section.icon} size={15} />
              {section.label}
            </button>
          </li>
        ))}
      </ul>

      {selfUpdate.value.kind === 'ready' ? (
        <div class="mt-auto flex flex-col gap-2 border-t border-rule px-5 py-4">
          <span class="font-mono text-eyebrow text-accent uppercase">v{selfUpdate.value.update.version} ready</span>
          <Button size="sm" variant="primary" icon={<Icon name="refresh" />} onClick={() => void applySelfUpdate()}>
            restart to update
          </Button>
        </div>
      ) : null}

      <div class={cx(selfUpdate.value.kind === 'ready' ? '' : 'mt-auto', 'border-t border-rule px-5 py-4')}>
        <span class="flex items-center gap-2">
          <span
            aria-hidden="true"
            class={cx('size-1.5 shrink-0', linked.length > 0 ? 'bg-ok' : failed.length > 0 ? 'bg-err' : 'bg-dim')}
          />
          <span class="truncate font-mono text-eyebrow text-muted uppercase">
            {linked.length > 0 ? `${linked.length} linked` : failed.length > 0 ? 'link failed' : 'no device'}
          </span>
        </span>
      </div>
    </nav>
  );
}
