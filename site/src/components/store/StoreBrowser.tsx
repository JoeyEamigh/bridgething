import { useEffect, useMemo, useState } from 'preact/hooks';
import { aggregate, countLine, failureLine, STORE_COPY } from '@bridgething/catalog';
import {
  fetchDirectory,
  fetchMergedApps,
  readStoreData,
  type DirectoryEntry,
  type InstallCount,
  type MergedCatalog,
} from '../../lib/directory-client';
import { orderedByTrust, sourceMap, vouchedFor } from '../../lib/store-sources';
import { AppSection } from './AppSection';
import { SourceDirectory } from './SourceDirectory';
import { SubmitSource } from './SubmitSource';

export function StoreBrowser({ initial }: { initial: MergedCatalog[] }) {
  const seeded = useMemo(readStoreData, []);

  const [catalogs, setCatalogs] = useState<MergedCatalog[]>(seeded?.apps.catalogs ?? initial);
  const [installs, setInstalls] = useState<InstallCount[]>(seeded?.apps.installs ?? []);
  const [failures, setFailures] = useState<{ url: string; reason: string }[]>(seeded?.apps.failures ?? []);
  const [directory, setDirectory] = useState<DirectoryEntry[] | null>(seeded?.directory ?? null);
  const [loading, setLoading] = useState(seeded === null);

  useEffect(() => {
    if (seeded) return;
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
  }, [seeded]);

  useEffect(() => {
    if (seeded) return;
    const controller = new AbortController();
    fetchDirectory({ signal: controller.signal })
      .then(setDirectory)
      .catch(() => setDirectory([]));
    return () => controller.abort();
  }, [seeded]);

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
        title={STORE_COPY.appsTitle}
        status={
          loading && vouched.length === 0
            ? 'loading…'
            : countLine(vouched.length, new Set(vouched.map(listing => listing.sourceUrl)).size)
        }
        empty={STORE_COPY.appsEmpty}
        listings={vouched}
        sources={sources}
      />

      {communitySources.length > 0 ? (
        <AppSection
          title={STORE_COPY.communityTitle}
          note={STORE_COPY.communityNote}
          status={countLine(community.length, communitySources.length)}
          empty={STORE_COPY.communityEmpty}
          listings={community}
          sources={sources}
        />
      ) : null}

      {failures.length > 0 ? (
        <div class="mb-16 border border-white/15 p-4">
          <p class="m-0 mb-2 font-mono text-sm text-white/45">{failureLine(failures.length)}</p>
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
