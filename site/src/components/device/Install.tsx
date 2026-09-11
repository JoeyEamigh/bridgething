import type { AppEntry, AppVersion } from '@bridgething/catalog';
import { newestCompatible, satisfies } from '@bridgething/catalog';
import {
  Button,
  Field,
  ListGroup,
  ListRow,
  Pill,
  SectionEmpty,
  SectionHeader,
  Segmented,
  type SegmentedOption,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { message } from '../../lib/browser-session';
import { useBrowser, useBrowserQuery, type BrowserBackend } from '../../lib/browser-tier';
import { fetchBundle, fetchCatalog } from '../../lib/catalog-source';
import { reportInstall } from '../../lib/directory-client';
import { isPlaceholderDownload, type PendingInstall } from '../../lib/pending-install';
import { ErrorNote, Hint, Section, bytes } from './Screen';
import { RunCard } from './Update';

type Source = 'catalog' | 'file';

const SOURCES: SegmentedOption<Source>[] = [
  { value: 'catalog', label: 'catalog url' },
  { value: 'file', label: '.zip' },
];

async function deliver(session: BrowserBackend, blob: Blob, provenance?: string): Promise<string> {
  const installed = await session.installWebappBytes(new Uint8Array(await blob.arrayBuffer()), provenance);
  return `${installed.name} v${installed.version}`;
}

function InstallProgress(): VNode | null {
  const runs = useBrowserQuery(['ota-runs'], s => s.otaRuns());
  const session = useBrowser();
  const run = (runs.data ?? []).find(entry => entry.kind === 'installedWebapp');
  if (!run) return null;
  return (
    <div class="mt-3">
      <RunCard
        run={run}
        onDismiss={() => {
          void session.dismissOtaRun();
        }}
      />
    </div>
  );
}

export function StagedInstall({
  pending,
  libVersion,
  serial,
  onDone,
}: {
  pending: PendingInstall;
  libVersion: string | null;
  serial: string | null;
  onDone: () => void;
}): VNode {
  const session = useBrowser();
  const [failure, setFailure] = useState<string | null>(null);
  const [note, setNote] = useState('starting');
  const started = useRef(false);

  useEffect(() => {
    if (started.current) return;
    started.current = true;

    void (async () => {
      if (isPlaceholderDownload(pending.download)) {
        setFailure('that catalog entry has no published bundle yet');
        return;
      }
      if (libVersion !== null && !satisfies(libVersion, pending.minLibbridgethingVersion)) {
        setFailure(`needs libbridgething ${pending.minLibbridgethingVersion}; this device runs ${libVersion}`);
        return;
      }

      setNote(`downloading from ${pending.provenance}`);
      const fetched = await fetchBundle(pending.download);
      if (!fetched.ok) {
        setFailure(fetched.message);
        return;
      }

      setNote('sha256 matches the catalog, sending it over');
      try {
        setNote(`installed ${await deliver(session, fetched.blob, pending.provenance)}`);
        reportInstall({
          appId: pending.appId,
          sourceUrl: pending.provenance,
          deviceId: serial,
          version: pending.version,
        });
        onDone();
      } catch (reason) {
        setFailure(message(reason));
      }
    })();
  }, [pending, libVersion, serial, session, onDone]);

  return (
    <Section>
      <SectionHeader title="from the store" />
      <ListGroup>
        <ListRow
          title={`${pending.name} ${pending.version}`}
          subtitle={failure ?? note}
          trailing={<Pill tone={failure ? 'err' : 'accent'}>{failure ? 'failed' : 'installing'}</Pill>}
        />
      </ListGroup>
      <InstallProgress />
    </Section>
  );
}

export function AddApp({ libVersion }: { libVersion: string | null }): VNode {
  const [source, setSource] = useState<Source>('catalog');

  return (
    <Section>
      <SectionHeader title="add an app" hint="anything that speaks catalog.v1, or a bundle you built yourself" />
      <Segmented<Source>
        class="mb-4 self-start"
        label="where the bundle comes from"
        options={SOURCES}
        value={source}
        onChange={setSource}
      />
      {source === 'catalog' ? <FromCatalog libVersion={libVersion} /> : <FromFile />}
      <InstallProgress />
    </Section>
  );
}

function FromCatalog({ libVersion }: { libVersion: string | null }): VNode {
  const [url, setUrl] = useState('');
  const [source, setSource] = useState('');
  const [apps, setApps] = useState<AppEntry[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const load = async () => {
    const trimmed = url.trim();
    if (!trimmed) return;
    setLoading(true);
    setFailure(null);
    setApps(null);

    const result = await fetchCatalog(trimmed);
    if (!result.ok) setFailure(result.message);
    else {
      setApps(result.catalog.apps);
      setSource(trimmed);
    }
    setLoading(false);
  };

  return (
    <div class="flex flex-col gap-3">
      <div class="flex items-end gap-3">
        <Field
          class="flex-1"
          label="catalog"
          type="url"
          placeholder="https://example.com/catalog.json"
          value={url}
          onInput={setUrl}
          onCommit={() => void load()}
        />
        <Button loading={loading} onClick={() => void load()}>
          load
        </Button>
      </div>

      {failure ? <ErrorNote>{failure}</ErrorNote> : null}

      {apps === null ? null : apps.length === 0 ? (
        <SectionEmpty>this source publishes no apps</SectionEmpty>
      ) : (
        <ListGroup>
          {apps.map(app => (
            <CatalogRow key={app.id} app={app} newest={newestCompatible(app, libVersion)} source={source} />
          ))}
        </ListGroup>
      )}
    </div>
  );
}

function CatalogRow({ app, newest, source }: { app: AppEntry; newest: AppVersion | null; source: string }): VNode {
  const session = useBrowser();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const placeholder = newest ? isPlaceholderDownload(newest.download) : false;
  const installable = newest !== null && !placeholder;

  const install = async () => {
    if (!newest) return;
    setBusy(true);
    setFailure(null);
    const fetched = await fetchBundle(newest.download);
    if (!fetched.ok) {
      setFailure(fetched.message);
      setBusy(false);
      return;
    }
    try {
      await deliver(session, fetched.blob, source);
    } catch (reason) {
      setFailure(message(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ListRow
      title={app.name}
      subtitle={failure ?? app.description}
      destructive={failure !== null}
      value={newest ? `v${newest.version}` : undefined}
      trailing={
        installable ? (
          <Button size="sm" loading={busy} onClick={() => void install()}>
            install
          </Button>
        ) : (
          <Pill tone="warn">{placeholder ? 'unpublished' : 'incompatible'}</Pill>
        )
      }
    />
  );
}

function FromFile(): VNode {
  const session = useBrowser();
  const [file, setFile] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const install = async () => {
    if (!file) return;
    setBusy(true);
    setFailure(null);
    setDone(null);
    try {
      setDone(`installed ${await deliver(session, file)}`);
    } catch (reason) {
      setFailure(message(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="flex flex-col gap-3">
      <input
        type="file"
        accept=".zip,application/zip"
        class="text-hint text-soft file:border-edge file:text-hint file:text-near font-mono file:mr-3 file:border file:bg-transparent file:px-3 file:py-1.5 file:font-mono"
        onChange={event => {
          setFile(event.currentTarget.files?.[0] ?? null);
          setFailure(null);
          setDone(null);
        }}
      />
      <Button variant="primary" class="self-start" loading={busy} disabled={!file} onClick={() => void install()}>
        install bundle
      </Button>
      {file ? <Hint>{`${file.name} · ${bytes(file.size)}`}</Hint> : null}
      {done ? <Hint>{done}</Hint> : null}
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </div>
  );
}
