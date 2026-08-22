import type { BridgethingLogArchive } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  FlatList,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
  Share,
  Text,
  View,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

import { ArmedButton } from '../components/ArmedButton';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { Field } from '../components/Field';
import { Icon, type IconName } from '../components/Icon';
import { LogArchiveSheet } from '../components/LogArchiveSheet';
import { Note } from '../components/Note';
import { Press } from '../components/Press';
import { Segmented } from '../components/Segmented';
import { Spinner } from '../components/Spinner';
import { clearLastCrash, formatCrash, useCrashStore } from '../lib/crash';
import {
  type DeviceLogLine,
  LOG_LIMIT,
  toLogLines,
  useDiagnostics,
  useMergedLogs,
} from '../lib/diagnostics';
import { getSession } from '../lib/session';
import { TEXT, type Tone, TYPE } from '../lib/theme';
import { logLevelTone, TONE_BG, TONE_TEXT } from '../lib/tone';
import { formatBytes, formatStamp } from '../lib/utils';
import type { SettingsScreenProps } from '../navigation';

type Props = SettingsScreenProps<'Logs'>;

const LEVELS = ['all', 'info', 'warn', 'error'] as const;
type LevelFilter = (typeof LEVELS)[number];

const SEVERITY: Record<string, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
};

const TAIL_SLOP_PX = 24;

const NO_LINES: DeviceLogLine[] = [];

const MESSAGE_TEXT = { fontSize: TYPE.hint, lineHeight: 16 };

type Notice = { tone: Tone; text: string };

export function LogsScreen(_: Props) {
  const entries = useMergedLogs();
  const deviceStreaming = useDiagnostics(s => s.deviceLogStreaming);
  const localStreaming = useDiagnostics(s => s.localLogStreaming);
  const setDeviceStreaming = useDiagnostics(s => s.setDeviceLogStreaming);
  const setLocalStreaming = useDiagnostics(s => s.setLocalLogStreaming);
  const clearLogs = useDiagnostics(s => s.clearDeviceLogs);
  const crash = useCrashStore(s => s.last);
  const streaming = deviceStreaming || localStreaming;

  const [filter, setFilter] = useState<LevelFilter>('all');
  const [query, setQuery] = useState('');
  const [storedBytes, setStoredBytes] = useState(0);
  const [atTail, setAtTail] = useState(true);
  const [archivesOpen, setArchivesOpen] = useState(false);
  const [clearOpen, setClearOpen] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [archive, setArchive] = useState<BridgethingLogArchive | null>(null);
  const [archiveLines, setArchiveLines] = useState<DeviceLogLine[] | null>(
    null,
  );
  const listRef = useRef<FlatList<DeviceLogLine>>(null);

  const source = archive ? (archiveLines ?? NO_LINES) : entries;

  const visible = useMemo(() => {
    const min = filter === 'all' ? -1 : SEVERITY[filter];
    const needle = query.trim().toLowerCase();
    const out: DeviceLogLine[] = [];
    for (let i = source.length - 1; i >= 0; i--) {
      const e = source[i];
      if (min >= 0 && (SEVERITY[e.level] ?? 0) < min) continue;
      if (needle && !e.message.toLowerCase().includes(needle)) continue;
      out.push(e);
    }
    return out;
  }, [source, filter, query]);

  const backToLive = useCallback(() => {
    setArchive(null);
    setArchiveLines(null);
  }, []);

  const openArchive = useCallback(
    (picked: BridgethingLogArchive) => {
      setArchive(picked);
      setArchiveLines(null);
      setArchivesOpen(false);
      setNotice(null);
      getSession()
        .logArchiveLines(picked.id, LOG_LIMIT)
        .then(lines => setArchiveLines(toLogLines(lines, `a${picked.id}-`)))
        .catch((err: unknown) => {
          backToLive();
          setNotice({ tone: 'err', text: describeError(err) });
        });
    },
    [backToLive],
  );

  const refreshStored = useCallback(() => {
    getSession()
      .persistedLogSize()
      .then(setStoredBytes)
      .catch(() => setStoredBytes(0));
  }, []);

  useEffect(refreshStored, [refreshStored]);

  const onScroll = useCallback((e: NativeSyntheticEvent<NativeScrollEvent>) => {
    setAtTail(e.nativeEvent.contentOffset.y <= TAIL_SLOP_PX);
  }, []);

  const jumpToTail = useCallback(() => {
    listRef.current?.scrollToOffset({ offset: 0, animated: true });
  }, []);

  const share = useCallback(async () => {
    setNotice(null);
    try {
      if (await getSession().shareLogs(archive?.id ?? null)) return;
    } catch {
      // fall through to the in-memory path
    }
    if (source.length === 0) {
      setNotice({ tone: 'neutral', text: 'no lines to share yet' });
      return;
    }
    try {
      await Share.share({ message: source.map(formatEntry).join('\n') });
    } catch (err) {
      setNotice({ tone: 'err', text: describeError(err) });
    }
  }, [archive, source]);

  const clearStored = useCallback(() => {
    setClearOpen(false);
    setNotice(null);
    clearLastCrash();
    getSession()
      .clearPersistedLogs()
      .catch((err: unknown) =>
        setNotice({ tone: 'err', text: describeError(err) }),
      )
      .finally(refreshStored);
  }, [refreshStored]);

  const shareCrash = useCallback(() => {
    if (!crash) return;
    void Share.share({ message: formatCrash(crash) }).catch(() => {});
  }, [crash]);

  return (
    <SafeAreaView edges={['bottom']} className="flex-1 bg-bg">
      <View className="gap-2.5 border-b border-rule bg-screen px-4 pb-2.5 pt-3">
        <Segmented
          options={LEVELS}
          value={filter}
          onChange={setFilter}
          size="sm"
        />

        <Field
          icon="Search"
          value={query}
          onChangeText={setQuery}
          placeholder="filter messages"
          autoCapitalize="none"
          autoCorrect={false}
          clearable
        />

        <View className="flex-row items-center gap-2">
          {archive ? (
            <ToolbarBtn icon="Radio" label="live" onPress={backToLive} />
          ) : (
            <>
              <ToolbarBtn
                icon={deviceStreaming ? 'Pause' : 'Play'}
                label="device"
                active={deviceStreaming}
                onPress={() => setDeviceStreaming(!deviceStreaming)}
              />
              <ToolbarBtn
                icon={localStreaming ? 'Pause' : 'Play'}
                label="phone"
                active={localStreaming}
                onPress={() => setLocalStreaming(!localStreaming)}
              />
            </>
          )}
          <ToolbarBtn
            icon="FolderClock"
            label="past launches"
            active={archive != null}
            onPress={() => setArchivesOpen(true)}
          />
        </View>

        <View className="flex-row items-center justify-between gap-3">
          <View className="flex-row items-center gap-2">
            <ToolbarBtn icon="Share2" onPress={() => void share()} />
            {archive ? null : (
              <ArmedButton
                label="clear"
                confirmLabel="tap again"
                icon="Trash2"
                size="sm"
                full={false}
                onConfirm={clearLogs}
              />
            )}
          </View>
          <Text
            className="min-w-0 flex-shrink font-mono text-muted"
            style={TEXT.eyebrow}
            numberOfLines={1}
          >
            {visible.length === source.length
              ? `${source.length} lines`
              : `${visible.length} of ${source.length} lines`}
            {archive
              ? ` · ${formatStamp(archive.startedAt)}`
              : streaming
                ? ' · streaming'
                : ' · stopped'}
          </Text>
        </View>

        {archive ? (
          source.length === LOG_LIMIT ? (
            <Text className="font-mono text-dim" style={TEXT.eyebrow}>
              share for the whole file
            </Text>
          ) : null
        ) : storedBytes > 0 ? (
          <Press
            onPress={() => setClearOpen(true)}
            hitSlop={8}
            className="self-start"
          >
            <Text className="font-mono text-muted" style={TEXT.eyebrow}>
              {formatBytes(storedBytes)} on disk ·{' '}
              <Text className="text-err">clear</Text>
            </Text>
          </Press>
        ) : null}
      </View>

      {crash ? (
        <View className="px-4 pt-2.5">
          <Note
            tone="err"
            title={`previous launch ${crash.fatal ? 'crashed' : 'hit an error'}`}
            action="share the details"
            onAction={shareCrash}
          >
            {crash.message}
          </Note>
        </View>
      ) : null}

      {notice ? (
        <View className="px-4 pt-2.5">
          <Note tone={notice.tone}>{notice.text}</Note>
        </View>
      ) : null}

      {archive && archiveLines === null ? (
        <View className="flex-1 items-center justify-center p-6">
          <Spinner />
        </View>
      ) : visible.length === 0 ? (
        <View className="flex-1 items-center justify-center p-6">
          <Text className="text-center font-sans text-muted" style={TEXT.body}>
            {emptyMessage(
              source.length,
              archive != null,
              streaming,
              query,
              filter,
            )}
          </Text>
        </View>
      ) : (
        <View className="flex-1">
          <FlatList
            ref={listRef}
            inverted
            data={visible}
            keyExtractor={keyExtractor}
            renderItem={renderRow}
            onScroll={onScroll}
            scrollEventThrottle={64}
            removeClippedSubviews
            initialNumToRender={24}
            maxToRenderPerBatch={24}
            windowSize={9}
            contentContainerClassName="px-4 py-2"
          />
          {atTail ? null : (
            <Press
              onPress={jumpToTail}
              className="absolute bottom-4 self-center flex-row items-center gap-1.5 border border-accent bg-accent-soft px-3 py-1.5"
            >
              <Icon name="ArrowDown" tone="accent" size={12} />
              <Text
                className="font-mono uppercase text-accent"
                style={TEXT.eyebrow}
              >
                latest
              </Text>
            </Press>
          )}
        </View>
      )}

      <LogArchiveSheet
        visible={archivesOpen}
        onClose={() => setArchivesOpen(false)}
        onChanged={refreshStored}
        onOpen={openArchive}
      />

      <ConfirmSheet
        visible={clearOpen}
        title="clear stored logs?"
        body="deletes all logs kept on disk, including error logs."
        confirmLabel="clear"
        destructive
        onConfirm={clearStored}
        onClose={() => setClearOpen(false)}
      />
    </SafeAreaView>
  );
}

function keyExtractor(e: DeviceLogLine): string {
  return e.id;
}

function renderRow({ item }: { item: DeviceLogLine }) {
  return <Row item={item} />;
}

const Row = memo(function Row({ item }: { item: DeviceLogLine }) {
  const { tag, body } = splitTag(item.message);
  const tone = logLevelTone(item.level);
  return (
    <View className="border-b border-rule py-1.5">
      <View className="flex-row items-center gap-2">
        <Text
          className={`px-1.5 font-mono uppercase ${TONE_BG[tone]} ${TONE_TEXT[tone]}`}
          style={TEXT.eyebrow}
        >
          {item.level.slice(0, 4)}
        </Text>
        <Text className="font-mono text-dim" style={TEXT.eyebrow}>
          {formatTime(item.ts)}
        </Text>
        {tag ? (
          <Text
            numberOfLines={1}
            className="flex-1 font-mono text-dim"
            style={TEXT.eyebrow}
          >
            {tag}
          </Text>
        ) : null}
      </View>
      <Text className="mt-0.5 font-mono text-fg" style={MESSAGE_TEXT}>
        {body}
      </Text>
    </View>
  );
});

function ToolbarBtn({
  icon: name,
  label,
  onPress,
  active,
}: {
  icon: IconName;
  label?: string;
  onPress: () => void;
  active?: boolean;
}) {
  return (
    <Press
      onPress={onPress}
      hitSlop={6}
      className={`h-8 flex-row items-center gap-1.5 border px-2 ${
        active ? 'border-accent bg-accent-soft' : 'border-rule'
      }`}
    >
      <Icon name={name} tone="accent" size={12} />
      {label ? (
        <Text className="font-mono uppercase text-accent" style={TEXT.eyebrow}>
          {label}
        </Text>
      ) : null}
    </Press>
  );
}

function splitTag(message: string): { tag: string | null; body: string } {
  const m = /^\[([^\]]{1,48})\]\s?([\s\S]*)$/.exec(message);
  return m ? { tag: m[1], body: m[2] } : { tag: null, body: message };
}

function emptyMessage(
  total: number,
  archived: boolean,
  streaming: boolean,
  query: string,
  filter: LevelFilter,
): string {
  if (total === 0) {
    if (archived) return 'nothing was recorded in this launch';
    return streaming
      ? 'streaming; no log lines yet'
      : 'press device or phone to stream logs. fair warning: streaming device logs will tank performance. turn off when done.';
  }
  if (query.trim()) return `no lines match "${query.trim()}"`;
  return `no lines at ${filter} or above`;
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return (
    String(d.getHours()).padStart(2, '0') +
    ':' +
    String(d.getMinutes()).padStart(2, '0') +
    ':' +
    String(d.getSeconds()).padStart(2, '0') +
    '.' +
    String(d.getMilliseconds()).padStart(3, '0')
  );
}

function formatEntry(e: DeviceLogLine): string {
  return `[${formatTime(e.ts)}] ${e.level.toUpperCase().padEnd(5)} ${e.message}`;
}
