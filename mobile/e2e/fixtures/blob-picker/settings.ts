import {
  BlobError,
  settings,
} from '../../../../packages/client-ts/dist/settings.js';

const pick = document.createElement('button');
pick.textContent = 'pick a gif';
pick.style.cssText = 'font-size: 24px; padding: 16px 32px; margin: 24px;';
const status = document.createElement('p');
document.body.append(pick, status);

pick.addEventListener('click', async () => {
  status.textContent = 'asking for a file';
  try {
    const picked = await settings.pick({ kind: 'image' });
    if (!picked) {
      status.textContent = 'nothing picked';
      return;
    }
    status.textContent = `picked ${picked.mime}`;
    let finished = false;
    const stored = await settings.blob.put('background', picked.handle, {
      onProgress: (sent, total) => (finished = sent === total),
    });
    status.textContent = `stored ${stored.mime} ${stored.sha256}${finished ? ' with progress' : ''}`;
  } catch (err) {
    status.textContent = `failed ${err instanceof BlobError ? err.kind : 'outright'}: ${String(err)}`;
  }
});
