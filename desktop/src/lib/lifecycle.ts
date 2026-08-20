import { invoke } from '@tauri-apps/api/core';

export const quit = (): Promise<void> => invoke('quit');
export const restart = (): Promise<void> => invoke('restart');
