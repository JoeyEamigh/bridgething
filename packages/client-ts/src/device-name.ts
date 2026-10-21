export type NamedDevice = { nickname?: string | null; bluetoothName?: string | null };

export function deviceName({ nickname, bluetoothName }: NamedDevice): string {
  return nickname || bluetoothName || 'Car Thing';
}
