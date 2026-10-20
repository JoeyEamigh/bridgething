import type { BridgethingClient } from '@bridgething/client';
import { wireErrorText } from './wire-error.ts';

export async function factoryReset(client: Pick<BridgethingClient, 'system'>): Promise<string | undefined> {
  const result = await client.system.factoryReset();
  if (result.ok) return undefined;
  return result.kind === 'domain' ? result.error.reason : wireErrorText(result.error);
}
