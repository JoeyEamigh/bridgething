import {
  installsLabel,
  listingKindLabel,
  type CatalogAppListing,
} from '@bridgething/catalog';
import { Text, View } from 'react-native';

import { useStoreInstall } from './install';
import { Button } from '../Button';
import { CatalogIcon } from '../CatalogIcon';
import { DeviceFrame } from '../DeviceFrame';
import { Pill } from '../Pill';
import { Press } from '../Press';
import { TEXT, TYPE } from '../../lib/theme';
import { listingState } from '../../lib/tone';

export const TILE_WIDTH = 72;
export const CARD_WIDTH = 220;

const CAPTION = { fontSize: TYPE.eyebrow };

export function byline(listing: CatalogAppListing): string {
  return [listing.app.author, installsLabel(listing.installs)]
    .filter(Boolean)
    .join(' · ');
}

export function GetButton({
  deviceId,
  listing,
  label = 'get',
  variant = 'secondary',
  onOpen,
}: {
  deviceId: string | null;
  listing: CatalogAppListing;
  label?: string;
  variant?: 'primary' | 'secondary';
  onOpen: () => void;
}) {
  const { installing, waiting, queued, failed, install } = useStoreInstall(
    deviceId,
    listing,
  );
  const state = listingState(listing);

  if (installing) {
    return (
      <View className="border border-accent px-3.5 py-1.5">
        <Text className="font-mono text-accent" style={TEXT.hint}>
          {installing.percent}%
        </Text>
      </View>
    );
  }
  if (state.label !== 'install') {
    return <Pill tone={state.tone}>{state.label}</Pill>;
  }
  return (
    <Button
      size="sm"
      full={false}
      variant={failed ? 'secondary' : variant}
      disabled={queued}
      loading={waiting != null && !queued}
      onPress={() =>
        deviceId ? void install(listing.newestCompatible) : onOpen()
      }
    >
      {queued ? 'queued' : failed ? 'retry' : label}
    </Button>
  );
}

export function ListingRow({
  listing,
  rank,
  deviceId,
  onOpen,
}: {
  listing: CatalogAppListing;
  rank?: number;
  deviceId: string | null;
  onOpen: () => void;
}) {
  return (
    <Press onPress={onOpen} testID={`store-row-${listing.app.name}`}>
      <View className="flex-row items-center gap-3 px-3 py-2.5">
        {rank != null ? (
          <Text
            className="font-mono text-dim"
            style={[TEXT.hint, { minWidth: 16 }]}
          >
            {rank}
          </Text>
        ) : null}
        <CatalogIcon url={listing.app.icon} name={listing.app.name} size={40} />
        <View className="min-w-0 flex-1">
          <Text
            className="font-sans text-fg"
            style={TEXT.row}
            numberOfLines={1}
          >
            {listing.app.name}
          </Text>
          <Text
            className="font-sans text-muted"
            style={TEXT.hint}
            numberOfLines={1}
          >
            {byline(listing)}
          </Text>
        </View>
        <GetButton deviceId={deviceId} listing={listing} onOpen={onOpen} />
      </View>
    </Press>
  );
}

export function ListingTile({
  listing,
  caption,
  onOpen,
}: {
  listing: CatalogAppListing;
  caption?: string | null;
  onOpen: () => void;
}) {
  return (
    <Press onPress={onOpen} style={{ width: TILE_WIDTH }}>
      <CatalogIcon url={listing.app.icon} name={listing.app.name} size={64} />
      <Text
        className="mt-1.5 font-sans text-fg"
        style={TEXT.hint}
        numberOfLines={1}
      >
        {listing.app.name}
      </Text>
      <Text className="font-mono text-dim" style={CAPTION} numberOfLines={1}>
        {caption ?? listingKindLabel(listing) ?? listing.app.author}
      </Text>
    </Press>
  );
}

export function ListingCard({
  listing,
  caption,
  onOpen,
}: {
  listing: CatalogAppListing;
  caption: string;
  onOpen: () => void;
}) {
  return (
    <Press onPress={onOpen} style={{ width: CARD_WIDTH }}>
      <DeviceFrame
        uri={listing.app.screenshots?.[0]}
        width={CARD_WIDTH}
        fallback={
          <CatalogIcon
            url={listing.app.icon}
            name={listing.app.name}
            size={56}
          />
        }
      />
      <View className="mt-2 flex-row items-center gap-2">
        <CatalogIcon url={listing.app.icon} name={listing.app.name} size={28} />
        <View className="min-w-0 flex-1">
          <Text
            className="font-sans text-fg"
            style={TEXT.hint}
            numberOfLines={1}
          >
            {listing.app.name}
          </Text>
          <Text
            className="font-mono text-dim"
            style={CAPTION}
            numberOfLines={1}
          >
            {caption}
          </Text>
        </View>
      </View>
    </Press>
  );
}
