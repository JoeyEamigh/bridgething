import type { BridgethingLogArchive } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useCallback, useEffect, useState } from 'react';
import { ScrollView, Text, View } from 'react-native';

import { getSession } from '../lib/session';
import { TEXT } from '../lib/theme';
import { formatBytes, formatStamp } from '../lib/utils';
import { Button } from './Button';
import { ConfirmBody } from './ConfirmSheet';
import { Icon } from './Icon';
import { Note } from './Note';
import { Press } from './Press';
import { Sheet, SheetTitle } from './Sheet';
import { Spinner } from './Spinner';

export function LogArchiveSheet({
  visible,
  onClose,
  onChanged,
  onOpen,
}: {
  visible: boolean;
  onClose: () => void;
  onChanged?: () => void;
  onOpen: (archive: BridgethingLogArchive) => void;
}) {
  const [archives, setArchives] = useState<BridgethingLogArchive[] | null>(
    null,
  );
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState<BridgethingLogArchive | null>(null);

  const refresh = useCallback(() => {
    getSession()
      .logArchives()
      .then(setArchives)
      .catch(() => setArchives([]));
  }, []);

  useEffect(() => {
    if (!visible) return;
    setArchives(null);
    setFailure(null);
    setPending(null);
    refresh();
  }, [visible, refresh]);

  const share = useCallback(async (archive: BridgethingLogArchive) => {
    setBusy(archive.id);
    setFailure(null);
    try {
      const ok = await getSession().shareLogs(archive.id);
      if (!ok) setFailure('no app on this phone can receive a log file');
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setBusy(null);
    }
  }, []);

  const remove = useCallback(
    async (archive: BridgethingLogArchive) => {
      setBusy(archive.id);
      setFailure(null);
      try {
        await getSession().deleteLogArchive(archive.id);
      } catch (err) {
        setFailure(describeError(err));
      } finally {
        setBusy(null);
        setPending(null);
        refresh();
        onChanged?.();
      }
    },
    [refresh, onChanged],
  );

  if (pending) {
    return (
      <Sheet visible={visible} onClose={onClose}>
        <ConfirmBody
          title="delete this log?"
          body={
            pending.current
              ? 'clears the log for this launch.'
              : `deletes the ${formatBytes(pending.bytes)} recorded on ${formatStamp(pending.startedAt)}.`
          }
          confirmLabel="delete"
          destructive
          busy={busy === pending.id}
          onConfirm={() => void remove(pending)}
          onCancel={() => setPending(null)}
        />
      </Sheet>
    );
  }

  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetTitle>past launches</SheetTitle>

      {archives === null ? (
        <View className="items-center py-8">
          <Spinner />
        </View>
      ) : archives.length === 0 ? (
        <Text
          className="py-6 text-center font-sans text-muted"
          style={TEXT.body}
        >
          nothing recorded on disk yet
        </Text>
      ) : (
        <ScrollView
          className="max-h-80"
          contentContainerClassName="gap-px"
          showsVerticalScrollIndicator={false}
        >
          {archives.map(archive => (
            <View
              key={archive.id}
              className="flex-row items-center gap-2 border border-rule bg-screen px-3 py-2.5"
            >
              <Press onPress={() => onOpen(archive)} className="flex-1">
                <View className="flex-row items-center gap-2">
                  <Text className="font-mono text-fg" style={TEXT.hint}>
                    {formatStamp(archive.startedAt)}
                  </Text>
                  {archive.current ? (
                    <Text
                      className="font-mono uppercase text-accent"
                      style={TEXT.eyebrow}
                    >
                      live
                    </Text>
                  ) : null}
                  {archive.pinned ? (
                    <Icon name="TriangleAlert" tone="err" size={12} />
                  ) : null}
                </View>
                <Text
                  className="mt-0.5 font-mono text-dim"
                  style={TEXT.eyebrow}
                >
                  {formatBytes(archive.bytes)}
                  {archive.pinned ? ' · kept: has errors' : ''} · tap to read
                </Text>
              </Press>

              {busy === archive.id ? (
                <Spinner />
              ) : (
                <>
                  <Press
                    onPress={() => void share(archive)}
                    hitSlop={8}
                    className="border border-rule p-2"
                  >
                    <Icon name="Share2" tone="accent" size={14} />
                  </Press>
                  <Press
                    onPress={() => setPending(archive)}
                    hitSlop={8}
                    className="border border-rule p-2"
                  >
                    <Icon name="Trash2" tone="err" size={14} />
                  </Press>
                </>
              )}
            </View>
          ))}
        </ScrollView>
      )}

      {failure ? <Note tone="err">{failure}</Note> : null}

      <Button variant="ghost" size="md" onPress={onClose}>
        done
      </Button>
    </Sheet>
  );
}
