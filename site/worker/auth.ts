export function bearerToken(request: Request): string | null {
  const header = request.headers.get('authorization') ?? '';
  const prefix = 'Bearer ';
  if (!header.startsWith(prefix)) return null;
  return header.slice(prefix.length).trim() || null;
}

export function tokenMatches(provided: string, expected: string): boolean {
  if (!expected) return false;
  const a = new TextEncoder().encode(provided);
  const b = new TextEncoder().encode(expected);
  let diff = a.byteLength ^ b.byteLength;
  for (let i = 0; i < Math.max(a.byteLength, b.byteLength); i += 1) {
    diff |= (a[i] ?? 0) ^ (b[i] ?? 0);
  }
  return diff === 0;
}

export function isAdmin(request: Request, adminToken: string | undefined): boolean {
  const token = bearerToken(request);
  return token !== null && tokenMatches(token, adminToken ?? '');
}
