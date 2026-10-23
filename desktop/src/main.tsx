import { SessionProvider } from '@bridgething/ui';
import { render } from 'preact';

import './app.css';
import { App } from './App.tsx';
import { autostart } from './stores/autostart.ts';
import { watchInstallCensus } from './stores/census.ts';
import { seed } from './stores/session.ts';
import { TauriSession } from './tauri-session.ts';

const root = document.getElementById('app');
if (!root) throw new Error('the shell template is missing its mount point');

const session = await TauriSession.start();
await seed(session);
void autostart.refresh();
watchInstallCensus();

render(
  <SessionProvider session={session}>
    <App />
  </SessionProvider>,
  root,
);
