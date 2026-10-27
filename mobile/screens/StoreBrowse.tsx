import {
  browse,
  queryTitle,
  SORT_LABELS,
  STORE_COPY,
  type StoreSort,
} from '@bridgething/catalog';
import { useMemo, useState } from 'react';
import { View } from 'react-native';

import { ListGroup } from '../components/ListGroup';
import { ScreenHeader } from '../components/ScreenHeader';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty } from '../components/SectionHeader';
import { Segmented } from '../components/Segmented';
import { ListingRow } from '../components/store/listing';
import { useStoreCatalog } from '../lib/catalog';
import type { StoreScreenProps } from '../navigation';

type Props = StoreScreenProps<'StoreBrowse'>;

const SORTS: StoreSort[] = ['popular', 'new', 'updated'];
const RANKED: ReadonlySet<StoreSort> = new Set(['popular', 'trending']);

export function StoreBrowseScreen({ navigation, route }: Props) {
  const { deviceId, query } = route.params;
  const [sort, setSort] = useState(query.sort);
  const { listings } = useStoreCatalog(deviceId);

  const sortable = query.filter !== null && query.filter.kind !== 'author';
  const current = { filter: query.filter, sort };
  const matched = useMemo(
    () => browse(listings, { filter: query.filter, sort }),
    [listings, query.filter, sort],
  );
  const ranked = RANKED.has(sort);

  return (
    <ScrollScreen>
      <ScreenHeader title={queryTitle(current)} />
      {sortable ? (
        <View className="mb-4">
          <Segmented<StoreSort>
            size="sm"
            options={SORTS.map(value => ({ value, label: SORT_LABELS[value] }))}
            value={sort}
            onChange={setSort}
          />
        </View>
      ) : null}
      {matched.length > 0 ? (
        <ListGroup>
          {matched.map((listing, index) => (
            <ListingRow
              key={listing.app.id}
              listing={listing}
              rank={ranked ? index + 1 : undefined}
              deviceId={deviceId}
              onOpen={() =>
                navigation.navigate('StoreApp', {
                  deviceId,
                  appId: listing.app.id,
                  sourceUrl: listing.sourceUrl,
                })
              }
            />
          ))}
        </ListGroup>
      ) : (
        <SectionEmpty>{STORE_COPY.empty}</SectionEmpty>
      )}
    </ScrollScreen>
  );
}
