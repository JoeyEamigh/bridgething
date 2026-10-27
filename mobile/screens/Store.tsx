import {
  failureLine,
  search,
  STORE_COPY,
  type BrowseQuery,
  type CatalogAppListing,
} from '@bridgething/catalog';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { RefreshControl, Text, View } from 'react-native';

import { Button } from '../components/Button';
import { ListGroup } from '../components/ListGroup';
import { Note } from '../components/Note';
import { ScreenHeader } from '../components/ScreenHeader';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { Spinner } from '../components/Spinner';
import { ListingRow } from '../components/store/listing';
import {
  ChipRow,
  FeaturedCarousel,
  SearchField,
  StoreShelf,
  type Chip,
} from '../components/store/storefront';
import {
  refreshCatalog,
  useCatalog,
  useStoreCatalog,
  useStorefront,
  useStoreReady,
} from '../lib/catalog';
import { useReachable } from '../lib/reachability';
import { connectedPeers, useSession } from '../lib/session';
import { TEXT, usePalette } from '../lib/theme';
import type { StoreScreenProps } from '../navigation';

type Props = StoreScreenProps<'Store'>;

export function StoreScreen({ navigation }: Props) {
  const peers = useSession(s => s.peers);
  const deviceId = connectedPeers(peers)[0]?.id ?? null;

  const { listings } = useStoreCatalog(deviceId);
  const { featured, shelves, filters, all } = useStorefront(listings);
  const ready = useStoreReady();
  const refreshing = useCatalog(s => s.refreshing);
  const failures = useCatalog(s => s.failures);
  const reachable = useReachable();
  const palette = usePalette();

  const [pulled, setPulled] = useState(false);
  const [query, setQuery] = useState('');

  useEffect(() => {
    void refreshCatalog();
  }, []);

  useEffect(() => {
    if (!refreshing) setPulled(false);
  }, [refreshing]);

  const onRefresh = useCallback(() => {
    setPulled(true);
    void refreshCatalog();
  }, []);

  const open = (listing: CatalogAppListing) =>
    navigation.navigate('StoreApp', {
      deviceId,
      appId: listing.app.id,
      sourceUrl: listing.sourceUrl,
    });

  const browseTo = (next: BrowseQuery) =>
    navigation.navigate('StoreBrowse', { deviceId, query: next });

  const results = useMemo(() => search(listings, query), [listings, query]);
  const searching = query.trim().length > 0;

  const chips: Chip[] = [
    { key: 'all', label: 'all', count: all.length },
    ...filters.map(entry => ({
      key: entry.key,
      label: entry.label,
      onPress: () => browseTo({ filter: entry.filter, sort: 'popular' }),
    })),
  ];

  const body = searching ? (
    <View className="mb-8">
      <SectionHeader title="results" />
      {results.length > 0 ? (
        <ListGroup>
          {results.map(listing => (
            <ListingRow
              key={listing.app.id}
              listing={listing}
              deviceId={deviceId}
              onOpen={() => open(listing)}
            />
          ))}
        </ListGroup>
      ) : (
        <SectionEmpty>{STORE_COPY.noMatches}</SectionEmpty>
      )}
    </View>
  ) : ready && listings.length > 0 ? (
    <>
      <ChipRow chips={chips} active="all" />
      {featured.length > 0 ? (
        <FeaturedCarousel
          listings={featured}
          deviceId={deviceId}
          onOpen={open}
        />
      ) : null}
      {shelves.map(shelf => (
        <StoreShelf
          key={shelf.id}
          shelf={shelf}
          deviceId={deviceId}
          onOpen={open}
          onSeeAll={() => browseTo(shelf.more)}
        />
      ))}
      <View className="mb-8" testID="store-all">
        <SectionHeader title="all apps" />
        <ListGroup>
          {all.map(listing => (
            <ListingRow
              key={listing.app.id}
              listing={listing}
              deviceId={deviceId}
              onOpen={() => open(listing)}
            />
          ))}
        </ListGroup>
      </View>
    </>
  ) : !reachable ? (
    <Note tone="warn" action="retry" onAction={() => void refreshCatalog()}>
      this phone is offline · the store needs a connection
    </Note>
  ) : !ready || refreshing ? (
    <View className="items-center py-10">
      <Spinner />
    </View>
  ) : (
    <SectionEmpty>{STORE_COPY.empty}</SectionEmpty>
  );

  return (
    <ScrollScreen
      refreshControl={
        <RefreshControl
          refreshing={pulled && refreshing}
          onRefresh={onRefresh}
          tintColor={palette.dim}
          colors={[palette.accent]}
          progressBackgroundColor={palette.screen}
        />
      }
    >
      <ScreenHeader title="store" />
      <SearchField value={query} onChangeText={setQuery} />
      {body}

      <Button
        onPress={() => navigation.navigate('StoreSources', { deviceId })}
        variant="secondary"
        icon="Link"
      >
        manage sources
      </Button>

      {failures.length > 0 ? (
        <Text className="mt-3 px-1 font-sans text-muted" style={TEXT.hint}>
          {failureLine(failures.length)}
        </Text>
      ) : null}
    </ScrollScreen>
  );
}
