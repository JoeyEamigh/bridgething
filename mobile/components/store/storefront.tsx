import {
  installsLabel,
  lastReleasedAt,
  listingKindLabel,
  type CatalogAppListing,
  type Shelf,
} from '@bridgething/catalog';
import type { ReactNode } from 'react';
import {
  ScrollView,
  Text,
  TextInput,
  useWindowDimensions,
  View,
} from 'react-native';

import { GetButton, ListingCard, ListingRow, ListingTile } from './listing';
import { CatalogIcon } from '../CatalogIcon';
import { DeviceFrame } from '../DeviceFrame';
import { Icon } from '../Icon';
import { ListGroup } from '../ListGroup';
import { Press } from '../Press';
import { SectionHeader } from '../SectionHeader';
import { SPACE, TEXT, usePalette } from '../../lib/theme';
import { relativeTime } from '../../lib/utils';

export function SearchField({
  value,
  onChangeText,
}: {
  value: string;
  onChangeText: (next: string) => void;
}) {
  const palette = usePalette();
  return (
    <View className="mb-3 flex-row items-center gap-2 border border-edge px-3">
      <Icon name="Search" size={18} color={palette.dim} />
      <TextInput
        testID="store-search"
        value={value}
        onChangeText={onChangeText}
        placeholder="search"
        placeholderTextColor={palette.dim}
        autoCapitalize="none"
        autoCorrect={false}
        returnKeyType="search"
        className="flex-1 py-2.5 font-mono text-fg"
        style={TEXT.body}
      />
      {value.length > 0 ? (
        <Press onPress={() => onChangeText('')} hitSlop={10}>
          <Text className="px-1 font-mono text-dim" style={TEXT.body}>
            ×
          </Text>
        </Press>
      ) : null}
    </View>
  );
}

export type Chip = { key: string; label: string; onPress?: () => void };

export function ChipRow({ chips, active }: { chips: Chip[]; active: string }) {
  return (
    <HorizontalRail gap={6} className="mb-6">
      {chips.map(chip => {
        const on = chip.key === active;
        return (
          <Press
            key={chip.key}
            onPress={chip.onPress}
            disabled={on}
            testID={`store-chip-${chip.key}`}
          >
            <View
              className={`border px-2.5 py-1 ${on ? 'border-accent bg-accent-soft' : 'border-rule'}`}
            >
              <Text
                className={`font-mono ${on ? 'text-accent' : 'text-soft'}`}
                style={TEXT.hint}
              >
                {chip.label}
              </Text>
            </View>
          </Press>
        );
      })}
    </HorizontalRail>
  );
}

export function HorizontalRail({
  children,
  gap = 12,
  className,
}: {
  children: ReactNode;
  gap?: number;
  className?: string;
}) {
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      style={{ marginHorizontal: -SPACE.gutter }}
      contentContainerStyle={{ gap, paddingHorizontal: SPACE.gutter }}
      className={className}
    >
      {children}
    </ScrollView>
  );
}

export function FeaturedHero({
  listing,
  deviceId,
  onOpen,
}: {
  listing: CatalogAppListing;
  deviceId: string | null;
  onOpen: () => void;
}) {
  const { width } = useWindowDimensions();
  const kind = listingKindLabel(listing);
  return (
    <View className="mb-8">
      <Press onPress={onOpen} testID="store-featured">
        <DeviceFrame
          uri={listing.app.screenshots?.[0]}
          width={width - SPACE.gutter * 2}
          fallback={
            <CatalogIcon
              url={listing.app.icon}
              name={listing.app.name}
              size={72}
            />
          }
        />
      </Press>
      <View className="mt-3 flex-row items-end justify-between gap-3">
        <Press onPress={onOpen} className="min-w-0 flex-1">
          <Text
            className="mb-1.5 font-mono uppercase text-accent"
            style={TEXT.eyebrow}
          >
            featured
          </Text>
          <Text
            className="font-display text-fg"
            style={TEXT.hero}
            numberOfLines={1}
          >
            {listing.app.name}
          </Text>
          <Text
            className="mt-0.5 font-sans text-muted"
            style={TEXT.hint}
            numberOfLines={1}
          >
            {[listing.app.author, kind].filter(Boolean).join(' · ')}
          </Text>
        </Press>
        <GetButton
          deviceId={deviceId}
          listing={listing}
          label="install"
          variant="primary"
          onOpen={onOpen}
        />
      </View>
    </View>
  );
}

function cardCaption(shelf: Shelf['id'], listing: CatalogAppListing): string {
  const kind = listingKindLabel(listing);
  switch (shelf) {
    case 'new':
      return kind ?? listing.app.author;
    case 'updated': {
      const at = lastReleasedAt(listing.app);
      const version = listing.newestCompatible?.version;
      return [version, at === null ? null : relativeTime(at)]
        .filter(Boolean)
        .join(' · ');
    }
    default:
      return installsLabel(listing.installs) ?? kind ?? listing.app.author;
  }
}

export function StoreShelf({
  shelf,
  deviceId,
  onOpen,
  onSeeAll,
}: {
  shelf: Shelf;
  deviceId: string | null;
  onOpen: (listing: CatalogAppListing) => void;
  onSeeAll: () => void;
}) {
  return (
    <View className="mb-8" testID={`store-shelf-${shelf.id}`}>
      <SectionHeader title={shelf.title} action="see all" onAction={onSeeAll} />
      {shelf.id === 'essentials' ? (
        <HorizontalRail>
          {shelf.listings.map(listing => (
            <ListingTile
              key={listing.app.id}
              listing={listing}
              onOpen={() => onOpen(listing)}
            />
          ))}
        </HorizontalRail>
      ) : shelf.id === 'popular' ? (
        <ListGroup>
          {shelf.listings.slice(0, 5).map((listing, index) => (
            <ListingRow
              key={listing.app.id}
              listing={listing}
              rank={index + 1}
              deviceId={deviceId}
              onOpen={() => onOpen(listing)}
            />
          ))}
        </ListGroup>
      ) : (
        <HorizontalRail>
          {shelf.listings.map(listing => (
            <ListingCard
              key={listing.app.id}
              listing={listing}
              caption={cardCaption(shelf.id, listing)}
              onOpen={() => onOpen(listing)}
            />
          ))}
        </HorizontalRail>
      )}
    </View>
  );
}
