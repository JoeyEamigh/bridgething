import type { WireError } from '@bridgething/client';

export function wireErrorText(error: WireError): string {
  switch (error.type) {
    case 'malformed':
      return `rejected by the device: ${error.data.reason}`;
    case 'handlerFailed':
      return `failed: ${error.data.reason}`;
    default:
      return `failed: ${error.type}`;
  }
}
