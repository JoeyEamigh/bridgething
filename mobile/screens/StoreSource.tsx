import { describeError } from '@bridgething/ui/errors';
import { useCallback, useEffect, useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from '../components/Button';
import { ListGroup } from '../components/ListGroup';
import { Note } from '../components/Note';
import { ScreenHeader } from '../components/ScreenHeader';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { Spinner } from '../components/Spinner';
import { ListingRow } from '../components/store/listing';
import {
  addSource,
  previewSource,
  removeSource,
  useIsSubscribed,
  usePreview,
  useSourceListings,
} from '../lib/catalog';
import { TEXT } from '../lib/theme';
import type { StoreScreenProps } from '../navigation';

type Props = StoreScreenProps<'StoreSource'>;

export function StoreSourceScreen({ route, navigation }: Props) {
  const { deviceId, url, name } = route.params;
  const subscribed = useIsSubscribed(url);
  const listings = useSourceListings(url, deviceId);
  const title = usePreview(url)?.repo.name ?? name;

  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    let cancelled = false;
    setLoading(true);
    previewSource(url)
      .then(() => {
        if (!cancelled) setError(null);
      })
      .catch(err => {
        if (!cancelled) setError(describeError(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [url]);

  useEffect(() => load(), [load]);

  const toggle = async () => {
    setBusy(true);
    setError(null);
    try {
      if (subscribed) await removeSource(url);
      else await addSource(url);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ScrollScreen>
      <ScreenHeader title={title} subtitle={url} />

      <View className="mb-8">
        <Button
          onPress={() => void toggle()}
          loading={busy}
          variant={subscribed ? 'secondary' : 'primary'}
          icon={subscribed ? 'Trash2' : 'Plus'}
        >
          {subscribed ? 'remove this source' : 'add this source'}
        </Button>
        <Text className="mt-2 px-1 font-sans text-muted" style={TEXT.hint}>
          {subscribed
            ? 'its apps get checked for updates.'
            : 'you can install from here without adding it.'}
        </Text>
      </View>

      <SectionHeader title="apps" />
      {error ? (
        <Note tone="err" action="retry" onAction={load}>
          {error}
        </Note>
      ) : loading && listings.length === 0 ? (
        <View className="items-center py-10">
          <Spinner />
        </View>
      ) : listings.length > 0 ? (
        <ListGroup>
          {listings.map(listing => (
            <ListingRow
              key={listing.app.id}
              listing={listing}
              deviceId={deviceId}
              onOpen={() =>
                navigation.navigate('StoreApp', {
                  deviceId,
                  appId: listing.app.id,
                  sourceUrl: url,
                })
              }
            />
          ))}
        </ListGroup>
      ) : (
        <SectionEmpty>this source publishes no apps</SectionEmpty>
      )}
    </ScrollScreen>
  );
}
