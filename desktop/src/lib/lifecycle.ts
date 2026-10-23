import { invoke } from '@tauri-apps/api/core';

export const quit = (): Promise<void> => invoke('quit');
