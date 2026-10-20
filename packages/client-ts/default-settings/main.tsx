import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { ConfigForm, closeSettings, type ConfigFormHandle } from '../src/form.js';
import { settings } from '../src/settings.js';

function DefaultSettings() {
  const form = useRef<ConfigFormHandle>(null);
  const [saving, setSaving] = useState(false);

  const close = async () => {
    setSaving(true);
    await closeSettings(form.current).catch(() => undefined);
    setSaving(false);
  };

  return (
    <main>
      <ConfigForm ref={form} />
      <button type="button" className="done" disabled={saving} onClick={() => void close()}>
        Done
      </button>
    </main>
  );
}

settings.onContextChanged(() => window.scrollTo(0, 0));

createRoot(document.getElementById('root')!).render(<DefaultSettings />);
