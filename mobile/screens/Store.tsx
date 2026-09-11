import { countLine, failureLine, STORE_COPY } from '@bridgething/catalog';
import { useCallback, useEffect, useState } from 'react';
import { RefreshControl, Text, View } from 'react-native';

import { Button } from '../components/Button';
import { CatalogRow } from '../components/CatalogRow';
import { ListGroup } from '../components/ListGroup';
import { Note } from '../components/Note';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { Spinner } from '../components/Spinner';
import { refreshCatalog, useCatalog, useStoreListings } from '../lib/catalog';
import { useReachable } from '../lib/reachability';
import { connectedPeers, useSession } from '../lib/session';
import { TEXT, usePalette } from '../lib/theme';
import type { StoreScreenProps } from '../navigation';
import { ScreenHeader } from '../components/ScreenHeader';

type Props = StoreScreenProps<'Store'>;

export function StoreScreen({ navigation }: Props) {
  const peers = useSession(s => s.peers);
  const deviceId = connectedPeers(peers)[0]?.id ?? null;

  const { vouched, community, sourceNames } = useStoreListings(deviceId);
  const refreshing = useCatalog(s => s.refreshing);
  const failures = useCatalog(s => s.failures);
  const reachable = useReachable();
  const palette = usePalette();

  const [pulled, setPulled] = useState(false);

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

  const openApp = (appId: string, sourceUrl: string) =>
    navigation.navigate('StoreApp', { deviceId, appId, sourceUrl });

  const openSource = (url: string) =>
    navigation.navigate('StoreSource', {
      deviceId,
      url,
      name: sourceNames[url] ?? url,
    });

  const empty = vouched.length === 0 && community.length === 0;

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
      <ScreenHeader title="store" subtitle="get your apps here" />

      <SectionHeader
        title={STORE_COPY.appsTitle}
        hint={countLine(vouched.length, Object.keys(sourceNames).length)}
      />

      {vouched.length > 0 ? (
        <ListGroup>
          {vouched.map(listing => (
            <CatalogRow
              key={listing.app.id}
              listing={listing}
              sourceName={sourceNames[listing.sourceUrl]}
              onPressSource={() => openSource(listing.sourceUrl)}
              onPress={() => openApp(listing.app.id, listing.sourceUrl)}
            />
          ))}
        </ListGroup>
      ) : empty && !reachable ? (
        <Note tone="warn" action="retry" onAction={() => void refreshCatalog()}>
          this phone is offline · the store needs a connection
        </Note>
      ) : empty && refreshing ? (
        <View className="items-center py-10">
          <Spinner />
        </View>
      ) : (
        <SectionEmpty>{STORE_COPY.appsEmpty}</SectionEmpty>
      )}

      <View className="mt-4">
        <Button
          onPress={() => navigation.navigate('StoreSources', { deviceId })}
          variant="secondary"
          icon="Link"
        >
          manage sources
        </Button>
      </View>

      {community.length > 0 ? (
        <View className="mt-8">
          <SectionHeader
            title={STORE_COPY.communityTitle}
            hint={STORE_COPY.communityHint}
          />
          <ListGroup>
            {community.map(listing => (
              <CatalogRow
                key={listing.app.id}
                listing={listing}
                sourceName={sourceNames[listing.sourceUrl]}
                onPressSource={() => openSource(listing.sourceUrl)}
                onPress={() => openApp(listing.app.id, listing.sourceUrl)}
              />
            ))}
          </ListGroup>
        </View>
      ) : null}

      {failures.length > 0 ? (
        <Text className="mt-3 px-1 font-sans text-muted" style={TEXT.hint}>
          {failureLine(failures.length)}
        </Text>
      ) : null}
    </ScrollScreen>
  );
}
