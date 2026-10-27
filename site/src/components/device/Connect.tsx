import { serialAvailable } from '@bridgething/browser';
import { Button, Field, Segmented, StatusStrip, type SegmentedOption } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { SERIAL_URL, message } from '../../lib/browser-session';
import { useBrowser } from '../../lib/browser-tier';
import type { PendingInstall } from '../../lib/pending-install';
import { DEFAULT_HOST } from '../../lib/wired';
import { ErrorNote, Hint, Section } from './Screen';

type Transport = 'cable' | 'bluetooth';

const TRANSPORTS: SegmentedOption<Transport>[] = [
  { value: 'cable', label: 'over the cable' },
  { value: 'bluetooth', label: 'over bluetooth' },
];

export function Connect({ pending }: { pending: PendingInstall | null }): VNode {
  const session = useBrowser();
  const [transport, setTransport] = useState<Transport>('cable');
  const [host, setHost] = useState(DEFAULT_HOST);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const serial = serialAvailable();

  const connect = () => {
    setBusy(true);
    setFailure(null);
    session
      .connect(transport === 'cable' ? host.trim() : SERIAL_URL)
      .catch((reason: unknown) => setFailure(message(reason)))
      .finally(() => setBusy(false));
  };

  return (
    <>
      {pending ? (
        <StatusStrip
          tone="accent"
          title={`${pending.name} ${pending.version} is staged`}
          subtitle={`from ${pending.provenance}. connect a device and it installs itself.`}
        />
      ) : (
        <StatusStrip
          tone="warn"
          title="nothing connected"
          subtitle="plug the thing into this computer over usb-c, or pick a peer you already paired"
        />
      )}

      <Section>
        <Segmented<Transport>
          class="mb-4 self-start"
          label="how to reach the device"
          options={TRANSPORTS}
          value={transport}
          onChange={setTransport}
          disabled={busy}
        />

        {transport === 'cable' ? (
          <Field
            class="max-w-md"
            label="host"
            value={host}
            onInput={setHost}
            onSubmit={() => {
              if (!busy) connect();
            }}
            hint="more than one thing on this machine? they announce as bridgething-<serial>.local."
            disabled={busy}
          />
        ) : (
          <Hint>
            the chooser lists every paired bluetooth peer that advertises the bridgething profile. pair it once in your
            operating system's bluetooth settings first.
          </Hint>
        )}

        <Button
          class="mt-5 self-start"
          variant="primary"
          size="lg"
          loading={busy}
          disabled={transport === 'bluetooth' && !serial}
          onClick={connect}>
          connect device
        </Button>

        {transport === 'bluetooth' && !serial ? (
          <Hint>this browser has no web serial. use a chromium browser on desktop or android.</Hint>
        ) : null}

        {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      </Section>
    </>
  );
}
