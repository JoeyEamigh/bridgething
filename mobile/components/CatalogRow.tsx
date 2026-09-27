import {
  alsoAvailableLabel,
  listingTraits,
  STORE_COPY,
  type CatalogAppListing,
} from '@bridgething/catalog';
import { Text, View } from 'react-native';

import { CatalogIcon } from './CatalogIcon';
import { Pill } from './Pill';
import { Press } from './Press';
import { TEXT } from '../lib/theme';
import { listingState } from '../lib/tone';

export function CatalogRow({
  listing,
  sourceName,
  onPress,
  onPressSource,
}: {
  listing: CatalogAppListing;
  sourceName?: string;
  onPress: () => void;
  onPressSource?: () => void;
}) {
  const { app, newestCompatible, installedVersion, alsoAvailableFrom } =
    listing;
  const state = listingState(listing);

  const meta = [
    newestCompatible
      ? `v${newestCompatible.version}`
      : STORE_COPY.needsFirmware,
    installedVersion ? `installed v${installedVersion}` : null,
    ...listingTraits(listing),
    alsoAvailableLabel(alsoAvailableFrom),
  ].filter(Boolean);

  return (
    <Press onPress={onPress}>
      <View className="flex-row items-center gap-3 px-4 py-3">
        <CatalogIcon url={app.icon} name={app.name} size={44} />
        <View className="min-w-0 flex-1">
          <Text
            className="font-sans text-fg"
            style={TEXT.row}
            numberOfLines={1}
          >
            {app.name}
          </Text>
          <Text
            className="mt-0.5 font-sans text-muted"
            style={TEXT.hint}
            numberOfLines={2}
          >
            {app.description}
          </Text>
          <View className="mt-1 flex-row items-center gap-1.5">
            {sourceName ? (
              <Press
                onPress={onPressSource}
                disabled={!onPressSource}
                hitSlop={6}
              >
                <Text
                  className={`font-mono ${onPressSource ? 'text-accent' : 'text-dim'}`}
                  style={TEXT.eyebrow}
                  numberOfLines={1}
                >
                  {sourceName}
                </Text>
              </Press>
            ) : null}
            <Text
              className="min-w-0 flex-1 font-mono text-dim"
              style={TEXT.eyebrow}
              numberOfLines={1}
            >
              {sourceName ? '· ' : ''}
              {meta.join(' · ')}
            </Text>
          </View>
        </View>
        <Pill tone={state.tone}>{state.label}</Pill>
        <Text className="font-mono text-dim" style={TEXT.body}>
          ›
        </Text>
      </View>
    </Press>
  );
}
