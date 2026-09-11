import { fillsSlot } from '@bridgething/catalog';
import {
  type BridgethingConfigEntry,
  type BridgethingConfigField,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useCallback, useEffect, useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from '../components/Button';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { Field } from '../components/Field';
import { Icon } from '../components/Icon';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { Note } from '../components/Note';
import { Pill } from '../components/Pill';
import { Press } from '../components/Press';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { Segmented } from '../components/Segmented';
import { SlotAction } from '../components/SlotAction';
import { Spinner } from '../components/Spinner';
import { Switch } from '../components/ui/switch';
import { WebappIcon } from '../components/WebappIcon';
import { useUpdates } from '../lib/catalog';
import {
  getSession,
  peerDisplayName,
  usePeer,
  useSession,
} from '../lib/session';
import { TEXT } from '../lib/theme';
import { humanizePermission } from '../lib/webapp-permissions';
import { useWebapps } from '../lib/webapps';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'WebappDetail'>;

export function WebappDetailScreen({ navigation, route }: Props) {
  const session = getSession();
  const { deviceId, id } = route.params;

  const peer = usePeer(deviceId);
  const ledger = useSession(s => s.ledger);

  const { list, active } = useWebapps(deviceId);
  const info = list.find(w => w.id === id) ?? null;
  const update =
    useUpdates(deviceId).find(
      u => u.appId.toLowerCase() === id.toLowerCase(),
    ) ?? null;

  const [entries, setEntries] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<'switch' | 'uninstall' | null>(null);
  const [askUninstall, setAskUninstall] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const loadConfig = useCallback(async () => {
    setLoadError(null);
    try {
      const config = await session.listWebappConfig(deviceId, id);
      setEntries(toMap(config));
    } catch (err) {
      setLoadError(describeError(err));
    }
  }, [deviceId, id, session]);

  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  const writeField = async (key: string, value: string) => {
    setConfigError(null);
    try {
      await session.setWebappConfigField(deviceId, id, key, value);
      setEntries(prev => ({ ...prev, [key]: value }));
    } catch (err) {
      setConfigError(describeError(err));
    }
  };

  const resetField = async (key: string) => {
    setConfigError(null);
    try {
      await session.deleteWebappConfigField(deviceId, id, key);
      const fresh = await session.listWebappConfig(deviceId, id);
      setEntries(toMap(fresh));
    } catch (err) {
      setConfigError(describeError(err));
    }
  };

  const switchActive = async () => {
    setBusy('switch');
    setActionError(null);
    try {
      await session.switchWebapp(deviceId, id);
    } catch (err) {
      setActionError(describeError(err));
    } finally {
      setBusy(null);
    }
  };

  const uninstall = async () => {
    setAskUninstall(false);
    setBusy('uninstall');
    setActionError(null);
    try {
      await session.uninstallWebapp(deviceId, id);
      navigation.goBack();
    } catch (err) {
      setActionError(describeError(err));
    } finally {
      setBusy(null);
    }
  };

  if (!info) {
    return (
      <ScrollScreen contentContainerStyle={{ paddingTop: 12 }}>
        <View className="mb-6 flex-row items-center gap-4 border border-rule bg-screen p-4">
          <View className="h-16 w-16 items-center justify-center border border-rule bg-neutral-soft">
            <Spinner />
          </View>
          <Text className="font-mono text-dim" style={TEXT.hint}>
            {id}
          </Text>
        </View>
        {loadError ? (
          <Note tone="err" action="retry" onAction={() => void loadConfig()}>
            {loadError}
          </Note>
        ) : null}
      </ScrollScreen>
    );
  }

  const builtin = info.source === 'builtin';
  const isActive = active?.id.toLowerCase() === info.id.toLowerCase();

  return (
    <ScrollScreen contentContainerStyle={{ paddingTop: 12 }}>
      <ConfirmSheet
        visible={askUninstall}
        title={`uninstall ${info.name}?`}
        body="it leaves your car thing now. you can install it again obv."
        detail={`detail: ${info.id} ${info.version}`}
        confirmLabel="uninstall"
        destructive
        busy={busy === 'uninstall'}
        onConfirm={() => void uninstall()}
        onClose={() => setAskUninstall(false)}
      />

      <View className="mb-6 flex-row items-center gap-4 border border-rule bg-screen p-4">
        <WebappIcon
          deviceId={deviceId}
          id={info.id}
          iconHash={info.iconHash}
          name={info.name}
          size={64}
        />
        <View className="min-w-0 flex-1">
          <Text
            className="font-display text-fg"
            style={TEXT.title}
            numberOfLines={2}
          >
            {info.name}
          </Text>
          <Text className="mt-0.5 font-mono text-muted" style={TEXT.hint}>
            v{info.version}
          </Text>
          <View className="mt-2 flex-row flex-wrap gap-1.5">
            {builtin ? (
              <Pill tone="neutral">built-in</Pill>
            ) : (
              <Pill tone="accent">installed</Pill>
            )}
            {peer ? (
              <Pill tone="neutral">{peerDisplayName(peer, ledger)}</Pill>
            ) : null}
          </View>
        </View>
      </View>

      {info.description ? (
        <Text className="mb-6 px-1 font-sans text-muted" style={TEXT.body}>
          {info.description}
        </Text>
      ) : null}

      {update ? (
        <Press
          onPress={() =>
            navigation.navigate('store', {
              screen: 'StoreApp',
              params: {
                deviceId,
                appId: update.appId,
                sourceUrl: update.sourceUrl,
              },
            })
          }
          className="mb-6"
        >
          <View className="flex-row items-center gap-3 border border-accent bg-accent-soft px-4 py-3">
            <Icon name="ArrowUpCircle" tone="accent" size={18} />
            <View className="flex-1">
              <Text className="font-sans text-fg" style={TEXT.hint}>
                update available
              </Text>
              <Text className="mt-0.5 font-mono text-muted" style={TEXT.hint}>
                v{update.installedVersion} → v{update.target.version}
              </Text>
            </View>
            <Text className="font-mono text-dim" style={TEXT.body}>
              ›
            </Text>
          </View>
        </Press>
      ) : null}

      <View className="mb-8 gap-2">
        <View className="flex-row gap-2">
          <View className="flex-1">
            {isActive ? (
              <View className="flex-row items-center justify-center gap-2 border border-ok bg-ok-soft px-5 py-2.5">
                <Icon name="Check" tone="ok" size={17} />
                <Text className="font-mono text-ok" style={TEXT.body}>
                  active
                </Text>
              </View>
            ) : (
              <Button
                onPress={switchActive}
                loading={busy === 'switch'}
                variant="primary"
                icon="Play"
              >
                switch to this
              </Button>
            )}
          </View>
          {!builtin ? (
            <View className="flex-1">
              <Button
                onPress={() => setAskUninstall(true)}
                loading={busy === 'uninstall'}
                variant="destructive"
                icon="Trash2"
              >
                uninstall
              </Button>
            </View>
          ) : null}
        </View>
        {actionError ? <Note tone="err">{actionError}</Note> : null}
      </View>

      {fillsSlot(info, 'launcher') || fillsSlot(info, 'overlay') ? (
        <View className="mb-8 gap-2">
          {fillsSlot(info, 'launcher') ? (
            <SlotAction deviceId={deviceId} id={info.id} slot="launcher" />
          ) : null}
          {fillsSlot(info, 'overlay') ? (
            <SlotAction deviceId={deviceId} id={info.id} slot="overlay" />
          ) : null}
        </View>
      ) : null}

      {info.settingsHash ? (
        <View className="mb-8">
          <Button
            onPress={() =>
              navigation.navigate('WebappSettings', {
                deviceId,
                id,
                name: info.name,
              })
            }
            variant="secondary"
            icon="SlidersHorizontal"
          >
            open {info.name} settings
          </Button>
        </View>
      ) : null}

      <View className="mb-8">
        <SectionHeader
          title="settings"
          hint={info.config.length > 0 ? 'changes save on commit' : undefined}
        />
        {info.config.length > 0 ? (
          <View className="gap-3">
            {info.config.map(field => (
              <ConfigEditor
                key={field.key}
                field={field}
                value={entries[field.key] ?? field.defaultValue ?? ''}
                onCommit={value => writeField(field.key, value)}
                onReset={() => resetField(field.key)}
              />
            ))}
          </View>
        ) : (
          <SectionEmpty>this app has no settings</SectionEmpty>
        )}
        {loadError ? (
          <Note
            className="mt-2"
            tone="err"
            action="retry"
            onAction={() => void loadConfig()}
          >
            {loadError}
          </Note>
        ) : null}
        {configError ? (
          <Note className="mt-2" tone="err">
            {configError}
          </Note>
        ) : null}
      </View>

      {info.permissions.length > 0 ? (
        <View>
          <SectionHeader
            title="what this app can do"
            hint="granted by installation"
          />
          <ListGroup>
            {info.permissions.map(p => {
              const meta = humanizePermission(p);
              return (
                <ListRow
                  key={p}
                  icon={meta.icon}
                  title={meta.title}
                  subtitle={meta.subtitle}
                />
              );
            })}
          </ListGroup>
        </View>
      ) : null}
    </ScrollScreen>
  );
}

function ConfigEditor({
  field,
  value,
  onCommit,
  onReset,
}: {
  field: BridgethingConfigField;
  value: string;
  onCommit: (value: string) => void;
  onReset: () => void;
}) {
  return (
    <View className="border border-rule bg-screen p-4">
      <View className="mb-2 flex-row items-center justify-between">
        <Text
          className="flex-1 font-mono uppercase text-muted"
          style={TEXT.eyebrow}
        >
          {field.label}
        </Text>
        {field.defaultValue !== undefined ? (
          <Press
            onPress={onReset}
            hitSlop={10}
            className="flex-row items-center gap-1"
          >
            <Icon name="RotateCcw" tone="accent" size={11} />
            <Text
              className="font-mono uppercase text-accent"
              style={TEXT.eyebrow}
            >
              reset
            </Text>
          </Press>
        ) : null}
      </View>
      <ConfigInput field={field} value={value} onCommit={onCommit} />
    </View>
  );
}

function ConfigInput({
  field,
  value,
  onCommit,
}: {
  field: BridgethingConfigField;
  value: string;
  onCommit: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);

  switch (field.kind) {
    case 'boolean': {
      const on = value === 'true';
      return (
        <View className="flex-row items-center justify-between">
          <Text className="font-sans text-fg" style={TEXT.body}>
            {on ? 'enabled' : 'disabled'}
          </Text>
          <Switch
            value={on}
            onValueChange={next => onCommit(next ? 'true' : 'false')}
          />
        </View>
      );
    }
    case 'enum':
      return (
        <Segmented
          options={field.choices ?? []}
          value={value}
          onChange={onCommit}
          size="sm"
        />
      );
    case 'number':
      return (
        <Field
          value={draft}
          onChangeText={setDraft}
          onCommit={onCommit}
          keyboardType="numeric"
        />
      );
    case 'secret':
    case 'string':
    default:
      return (
        <Field
          value={draft}
          onChangeText={setDraft}
          onCommit={onCommit}
          autoCapitalize="none"
          autoCorrect={false}
          secureTextEntry={field.kind === 'secret'}
        />
      );
  }
}

function toMap(entries: BridgethingConfigEntry[]): Record<string, string> {
  const map: Record<string, string> = {};
  for (const e of entries) map[e.key] = e.value;
  return map;
}
