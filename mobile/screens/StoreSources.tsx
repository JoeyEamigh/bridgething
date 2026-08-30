import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from '../components/Button';
import { Field } from '../components/Field';
import { Icon } from '../components/Icon';
import { ListGroup } from '../components/ListGroup';
import { Note } from '../components/Note';
import { Pill } from '../components/Pill';
import { Press } from '../components/Press';
import { ScreenHeader } from '../components/ScreenHeader';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import {
  addSource,
  moveSource,
  parseSourceInput,
  useCatalog,
  useQuickAddSources,
} from '../lib/catalog';
import { TEXT } from '../lib/theme';
import type { StoreScreenProps } from '../navigation';

type Props = StoreScreenProps<'StoreSources'>;

export function StoreSourcesScreen({ route, navigation }: Props) {
  const deviceId = route.params?.deviceId ?? null;

  const sources = useCatalog(s => s.sources);
  const suggested = useQuickAddSources();

  const [draft, setDraft] = useState('');
  const [failure, setFailure] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  const open = (url: string, name: string) =>
    navigation.navigate('StoreSource', { deviceId, url, name });

  const subscribe = async (raw: string, name?: string) => {
    const parsed = parseSourceInput(raw);
    if (!parsed.ok) {
      setFailure(parsed.error);
      return;
    }
    setAdding(true);
    setFailure(null);
    try {
      const added = await addSource(parsed.url);
      setDraft('');
      open(added, name ?? added);
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setAdding(false);
    }
  };

  const browseFirst = () => {
    const parsed = parseSourceInput(draft);
    if (!parsed.ok) {
      setFailure(parsed.error);
      return;
    }
    setFailure(null);
    open(parsed.url, parsed.url);
  };

  return (
    <ScrollScreen>
      <ScreenHeader
        title="sources"
        subtitle="every app in the store comes from one."
      />

      <View className="mb-8">
        <SectionHeader title="add a source" />
        <Field
          testID="source-url"
          label="source url"
          icon="Link"
          value={draft}
          onChangeText={text => {
            setDraft(text);
            setFailure(null);
          }}
          clearable
          placeholder="example.com or https://example.com/catalog.json"
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="url"
        />
        {failure ? (
          <Note className="mt-2" tone="err">
            {failure}
          </Note>
        ) : null}
        <View className="mt-3 gap-2">
          <Button
            onPress={() => void subscribe(draft)}
            loading={adding}
            disabled={draft.trim().length === 0}
            icon="Plus"
          >
            add this source
          </Button>
          <Button
            onPress={browseFirst}
            disabled={draft.trim().length === 0 || adding}
            variant="ghost"
          >
            browse it first
          </Button>
        </View>
        <Text className="mt-2 px-1 font-sans text-muted" style={TEXT.hint}>
          a source gives you more apps to choose from. nothing here is reviewed.
        </Text>
      </View>

      <View className="mb-8">
        <SectionHeader
          title="your sources"
          hint={sources.length > 1 ? 'higher wins' : 'tap one to browse it'}
        />
        {sources.length === 0 ? (
          <SectionEmpty>no sources yet</SectionEmpty>
        ) : (
          <ListGroup>
            {sources.map((url, index) => (
              <SourceRow
                key={url}
                url={url}
                first={index === 0}
                last={index === sources.length - 1}
                reorderable={sources.length > 1}
                onPress={() => open(url, url)}
              />
            ))}
          </ListGroup>
        )}
      </View>

      {suggested.length > 0 ? (
        <View>
          <SectionHeader title="suggested" hint="add one, or tap to browse" />
          <ListGroup>
            {suggested.map(source => (
              <View
                key={source.url}
                className="flex-row items-center gap-3 px-4 py-3"
              >
                <Press
                  onPress={() => open(source.url, source.name)}
                  className="flex-1"
                >
                  <View className="flex-row items-center gap-3">
                    <View className="min-w-0 flex-1">
                      <View className="flex-row items-center gap-2">
                        <Text
                          className="flex-shrink font-sans text-fg"
                          style={TEXT.row}
                          numberOfLines={1}
                        >
                          {source.name}
                        </Text>
                        {source.attested ? (
                          <Pill tone="ok">attested</Pill>
                        ) : null}
                      </View>
                      <Text
                        className="mt-0.5 font-sans text-muted"
                        style={TEXT.hint}
                        numberOfLines={2}
                      >
                        {source.description ?? source.url}
                      </Text>
                    </View>
                    <Text className="font-mono text-dim" style={TEXT.body}>
                      ›
                    </Text>
                  </View>
                </Press>
                <Press
                  onPress={() => void subscribe(source.url, source.name)}
                  disabled={adding}
                  hitSlop={10}
                >
                  <View className="border border-edge p-2">
                    <Icon name="Plus" tone="accent" size={16} />
                  </View>
                </Press>
              </View>
            ))}
          </ListGroup>
          <Text className="mt-2 px-1 font-sans text-muted" style={TEXT.hint}>
            listed in the bridgething directory. apps are not reviewed.
          </Text>
        </View>
      ) : null}
    </ScrollScreen>
  );
}

function SourceRow({
  url,
  first,
  last,
  reorderable,
  onPress,
}: {
  url: string;
  first: boolean;
  last: boolean;
  reorderable: boolean;
  onPress: () => void;
}) {
  return (
    <View className="flex-row items-center gap-3 px-4 py-3">
      <Press onPress={onPress} className="flex-1">
        <View className="flex-row items-center gap-3">
          <Text
            className="flex-1 font-mono text-fg"
            style={TEXT.hint}
            numberOfLines={1}
          >
            {url}
          </Text>
          <Text className="font-mono text-dim" style={TEXT.body}>
            ›
          </Text>
        </View>
      </Press>
      {reorderable ? (
        <View className="flex-row items-center">
          <Press
            onPress={() => moveSource(url, -1)}
            disabled={first}
            hitSlop={8}
          >
            <View className="px-1.5 py-1">
              <Icon
                name="ChevronUp"
                size={16}
                tone={first ? 'neutral' : 'accent'}
              />
            </View>
          </Press>
          <Press onPress={() => moveSource(url, 1)} disabled={last} hitSlop={8}>
            <View className="px-1.5 py-1">
              <Icon
                name="ChevronDown"
                size={16}
                tone={last ? 'neutral' : 'accent'}
              />
            </View>
          </Press>
        </View>
      ) : null}
    </View>
  );
}
