import { useEffect, useMemo, useState } from 'preact/hooks';
import { aggregate } from '@bridgething/catalog';
import {
  fetchDirectory,
  fetchMergedApps,
  type DirectoryEntry,
  type InstallCount,
  type MergedCatalog,
} from '../../lib/directory-client';
import { orderedByTrust, sourceMap, vouchedFor } from '../../lib/store-sources';
import { AppSection } from './AppSection';
import { SourceDirectory } from './SourceDirectory';
import { SubmitSource } from './SubmitSource';

function countLine(listings: number, sources: number): string {
  return `${listings} across ${sources} source${sources === 1 ? '' : 's'}`;
}

export function StoreBrowser({ initial }: { initial: MergedCatalog[] }) {
  const [catalogs, setCatalogs] = useState<MergedCatalog[]>(initial);
  const [installs, setInstalls] = useState<InstallCount[]>([]);
  const [failures, setFailures] = useState<{ url: string; reason: string }[]>([]);
  const [directory, setDirectory] = useState<DirectoryEntry[] | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const controller = new AbortController();
    fetchMergedApps({ origin: '', signal: controller.signal })
      .then(merged => {
        setCatalogs(merged.catalogs);
        setInstalls(merged.installs);
        setFailures(merged.failures);
      })
      .catch(() => undefined)
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    fetchDirectory({ signal: controller.signal })
      .then(setDirectory)
      .catch(() => setDirectory([]));
    return () => controller.abort();
  }, []);

  const ordered = useMemo(() => orderedByTrust(catalogs), [catalogs]);

  const sources = useMemo(() => sourceMap(ordered), [ordered]);
  const vouchedUrls = useMemo(() => new Set(ordered.filter(vouchedFor).map(entry => entry.url)), [ordered]);

  const listings = useMemo(
    () =>
      aggregate({ orderedCatalogs: ordered, installed: [], deviceLibVersion: null, installs, extensions: 'listed' }),
    [ordered, installs],
  );

  const vouched = listings.filter(listing => vouchedUrls.has(listing.sourceUrl));
  const community = listings.filter(listing => !vouchedUrls.has(listing.sourceUrl));
  const communitySources = ordered.filter(entry => !vouchedFor(entry));

  return (
    <>
      <AppSection
        title="apps"
        status={
          loading && vouched.length === 0
            ? 'loading…'
            : countLine(vouched.length, new Set(vouched.map(listing => listing.sourceUrl)).size)
        }
        empty="the catalog exists but has no apps in it."
        listings={vouched}
        sources={sources}
      />

      {communitySources.length > 0 ? (
        <AppSection
          title="community apps"
          note="from sources in the directory that are listed but unreviewed."
          status={countLine(community.length, communitySources.length)}
          empty="these sources are listed but have no apps in them right now."
          listings={community}
          sources={sources}
        />
      ) : null}

      {failures.length > 0 ? (
        <div class="mb-16 border border-white/15 p-4">
          <p class="m-0 mb-2 font-mono text-sm text-white/45">
            {failures.length} source{failures.length === 1 ? '' : 's'} could not be read.
          </p>
          <ul class="m-0 flex list-none flex-col gap-1 p-0">
            {failures.map(failure => (
              <li key={failure.url} class="text-warn font-mono text-xs break-all">
                {failure.reason}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <SourceDirectory directory={directory} />

      <SubmitSource
        onSubmitted={entry =>
          setDirectory(current => [...(current ?? []).filter(existing => existing.url !== entry.url), entry])
        }
      />
    </>
  );
}
