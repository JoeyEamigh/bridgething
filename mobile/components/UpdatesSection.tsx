import { useEffect, useState } from 'react';
import { View } from 'react-native';

import { Field } from './Field';
import { ListGroup } from './ListGroup';
import { ListRow } from './ListRow';
import { SectionHeader } from './SectionHeader';
import { Switch } from './ui/switch';
import { rootUrlOf } from '../lib/ota';
import { patchOtaPollConfig, useSession } from '../lib/session';
import { DEFAULT_OTA_POLL_CONFIG, DEFAULT_OTA_ROOT_URL } from '../lib/storage';

export function UpdatesSection() {
  const pollConfig = useSession(s => s.otaPollConfig);
  const heldHost = pollConfig?.rootUrl ?? '';
  const [hostDraft, setHostDraft] = useState(heldHost);

  useEffect(() => setHostDraft(heldHost), [heldHost]);

  const commitHost = (raw: string) => {
    const next = raw.trim().replace(/\/+$/, '');
    if (next === rootUrlOf(pollConfig)) return;
    setHostDraft(next);
    void patchOtaPollConfig({ rootUrl: next || undefined });
  };

  return (
    <View className="mb-8">
      <SectionHeader
        title="updates"
        hint="how this phone keeps every car thing current."
      />
      <ListGroup>
        <ListRow
          icon="ArrowDownToLine"
          title="install updates automatically"
          subtitle="firmware and store apps"
          trailing={
            <Switch
              value={pollConfig?.autoPush ?? DEFAULT_OTA_POLL_CONFIG.autoPush}
              onValueChange={autoPush => patchOtaPollConfig({ autoPush })}
            />
          }
        />
      </ListGroup>
      <View className="mt-3">
        <Field
          label="update host"
          icon="Globe"
          testID="update-host"
          value={hostDraft}
          onChangeText={setHostDraft}
          onCommit={commitHost}
          placeholder={DEFAULT_OTA_ROOT_URL}
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="url"
          hint={`probably don't change this. leave empty for ${DEFAULT_OTA_ROOT_URL}.`}
          clearable
        />
      </View>
    </View>
  );
}
