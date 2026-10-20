import { PICK_BUSY } from '@bridgething/client/settings-protocol';
import type { MediaCover, MediaPickKind, PickedMedia } from '@bridgething/companion-types';
import { open, save } from '@tauri-apps/plugin-dialog';

import type { DesktopSession } from '../desktop.ts';

export type ArtifactKind = 'daemon' | 'webapp';

const FILTERS: Record<ArtifactKind, { name: string; extensions: string[] }> = {
  daemon: { name: 'daemon or system image', extensions: ['swu', 'zst', 'bin', '*'] },
  webapp: { name: 'webapp bundle', extensions: ['zip'] },
};

export async function pickArtifact(kind: ArtifactKind): Promise<string | null> {
  const picked = await open({
    multiple: false,
    directory: false,
    title: kind === 'daemon' ? 'pick a daemon or image artifact' : 'pick a webapp bundle',
    filters: [FILTERS[kind]],
  });
  return typeof picked === 'string' ? picked : null;
}

const MEDIA_FILTERS: Record<Exclude<MediaPickKind, 'any'>, { name: string; extensions: string[] }> = {
  image: {
    name: 'image',
    extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'heic', 'heif', 'bmp', 'tif', 'tiff'],
  },
  video: { name: 'video', extensions: ['mp4', 'mov', 'm4v', 'mkv', 'webm', 'avi'] },
};

export async function pickMediaFile(kind: MediaPickKind): Promise<string | null> {
  const picked = await open({
    multiple: false,
    directory: false,
    title: kind === 'any' ? 'pick a file' : `pick ${kind === 'image' ? 'an image' : 'a video'}`,
    filters: kind === 'any' ? [] : [MEDIA_FILTERS[kind]],
  });
  return typeof picked === 'string' ? picked : null;
}

export type MediaImports = Pick<DesktopSession, 'mediaImportTicket' | 'importPickedMedia' | 'cancelMediaImport'>;

type Flight = { abandoned: boolean; choosing: boolean; ticket: number | null };

let flight: Flight | null = null;

export async function pickMedia(
  imports: MediaImports,
  kind: MediaPickKind,
  cover: MediaCover | null,
): Promise<PickedMedia | null> {
  if (flight) throw new Error(PICK_BUSY);
  const mine: Flight = { abandoned: false, choosing: true, ticket: null };
  flight = mine;
  try {
    const path = await pickMediaFile(kind);
    mine.choosing = false;
    if (path === null || mine.abandoned) return null;
    const ticket = await imports.mediaImportTicket();
    mine.ticket = ticket;
    if (mine.abandoned) {
      await imports.cancelMediaImport(ticket);
      return null;
    }
    const media = await imports.importPickedMedia(ticket, path, kind, cover);
    return mine.abandoned ? null : media;
  } catch (err) {
    if (mine.abandoned) return null;
    throw err;
  } finally {
    if (flight === mine) flight = null;
  }
}

export function abandonPick(imports: MediaImports): void {
  const mine = flight;
  if (!mine || mine.abandoned) return;
  mine.abandoned = true;
  if (mine.ticket !== null) void imports.cancelMediaImport(mine.ticket);
  if (!mine.choosing) flight = null;
}

export async function pickLogFile(defaultPath: string): Promise<string | null> {
  return await save({
    title: 'save the log lines',
    defaultPath,
    filters: [{ name: 'log', extensions: ['log', 'txt'] }],
  });
}
