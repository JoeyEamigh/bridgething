import type { AppEntry, AppExtension, AppVersion } from './types.ts';

export const EXTENSION_PERMISSION_PATTERN = /^(?:all|(?:net|read|write|run|env|sys|ffi)(?::[^,]+)?)$/;

export const EXTENSION_SOURCE_PATTERN = /^https:\/\/github\.com\/[^/]+\/[^/]+\/?$/;

export function isExtensionPermission(permission: string): boolean {
  return EXTENSION_PERMISSION_PATTERN.test(permission);
}

export function extensionOf(version: AppVersion | null | undefined): AppExtension | null {
  return version?.extension ?? null;
}

export function declaresExtension(app: AppEntry): boolean {
  return app.versions.some(version => extensionOf(version) !== null);
}

export function describeExtensionPermission(permission: string): string {
  const cut = permission.indexOf(':');
  const verb = cut === -1 ? permission : permission.slice(0, cut);
  const scope = cut === -1 ? null : permission.slice(cut + 1);

  switch (verb) {
    case 'all':
      return scope === null ? 'full host access' : permission;
    case 'net':
      return scope === null ? 'reach any network host' : `reach ${scope}`;
    case 'read':
      return scope === null ? 'read any file' : `read ${scope}`;
    case 'write':
      return scope === null ? 'write any file' : `write ${scope}`;
    case 'run':
      return scope === null ? 'run any program' : `run ${scope}`;
    case 'env':
      return scope === null ? 'read every environment variable' : `read the ${scope} environment variable`;
    case 'sys':
      return scope === null ? 'read system information' : `read system ${scope}`;
    case 'ffi':
      return scope === null ? 'load any native library' : `load the native library ${scope}`;
    default:
      return permission;
  }
}

export function extensionRepoLabel(source: string | null | undefined): string | null {
  if (!source) return null;
  if (!EXTENSION_SOURCE_PATTERN.test(source)) return source;
  return source.replace(/^https:\/\/github\.com\//, '').replace(/\/$/, '');
}

export function describeExtensionPermissions(extension: AppExtension): string[] {
  if (extension.permissions.length === 0) return ['nothing beyond talking to your Car Thing'];
  return extension.permissions.map(describeExtensionPermission);
}
