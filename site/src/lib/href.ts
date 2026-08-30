const WEB_PROTOCOLS = ['http:', 'https:'];

export function webHref(value: string | null | undefined): string | null {
  if (typeof value !== 'string') return null;

  const raw = value.trim();
  if (raw.length === 0) return null;

  try {
    return WEB_PROTOCOLS.includes(new URL(raw).protocol) ? raw : null;
  } catch {
    return null;
  }
}
