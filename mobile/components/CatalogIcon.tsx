import { useEffect, useState } from 'react';
import { Image, Text, View } from 'react-native';
import { createMMKV } from 'react-native-mmkv';
import { SvgXml } from 'react-native-svg';

import { iconStore, rasterByPath, type IconBody } from '../lib/icon-cache';
import { TYPE } from '../lib/theme';

const ICON_MAX_BYTES = 64 * 1024;
const ICON_FETCH_TIMEOUT_MS = 10_000;
const MAX_INFLIGHT_FETCHES = 8;

const icons = iconStore(createMMKV({ id: 'bridgething.icons' }));
const failed = new Set<string>();
const pending = new Map<string, Promise<IconBody | null>>();

let inflight = 0;
const waiting: (() => void)[] = [];

async function acquireFetchSlot(): Promise<void> {
  if (inflight < MAX_INFLIGHT_FETCHES) {
    inflight += 1;
    return;
  }
  await new Promise<void>(resolve => waiting.push(resolve));
  inflight += 1;
}

function releaseFetchSlot(): void {
  inflight -= 1;
  waiting.shift()?.();
}

function looksLikeSvg(contentType: string | null, body: string): boolean {
  if (contentType && /(^|[/+])svg/i.test(contentType)) return true;
  return /^\s*(<\?xml[^>]*\?>\s*)?(<!--.*?-->\s*)*<svg[\s/>]/is.test(
    body.slice(0, 512),
  );
}

async function sniff(url: string): Promise<IconBody> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), ICON_FETCH_TIMEOUT_MS);
  try {
    const res = await fetch(url, { signal: abort.signal });
    if (!res.ok) throw new Error(`icon fetch failed (${res.status})`);
    const declared = Number(res.headers.get('content-length'));
    if (Number.isFinite(declared) && declared > ICON_MAX_BYTES) {
      throw new Error('icon larger than 64 KiB');
    }
    const type = res.headers.get('content-type');
    const undecided = !type || /octet-stream|^text\/plain/i.test(type);
    const svgByHeader = !!type && /(^|[/+])svg/i.test(type);
    if (!svgByHeader && !undecided) return { kind: 'raster' };
    const body = await res.text();
    if (body.length > ICON_MAX_BYTES)
      throw new Error('icon larger than 64 KiB');
    return looksLikeSvg(type, body)
      ? { kind: 'svg', xml: body }
      : { kind: 'raster' };
  } finally {
    clearTimeout(timer);
  }
}

function resolveIcon(url: string): Promise<IconBody | null> {
  const held = pending.get(url);
  if (held) return held;
  const next = (async () => {
    await acquireFetchSlot();
    try {
      const body = await sniff(url);
      icons.set(url, body);
      return body;
    } catch {
      return null;
    } finally {
      releaseFetchSlot();
      pending.delete(url);
    }
  })();
  pending.set(url, next);
  return next;
}

function known(url: string): { body: IconBody; fresh: boolean } | null {
  if (rasterByPath(url)) return { body: { kind: 'raster' }, fresh: true };
  return icons.get(url);
}

export function CatalogIcon({
  url,
  name,
  size,
}: {
  url: string | null;
  name: string;
  size: number;
}) {
  const [body, setBody] = useState<IconBody | null>(() =>
    url && !failed.has(url) ? (known(url)?.body ?? null) : null,
  );

  const [painted, setPainted] = useState<string | null>(null);

  useEffect(() => {
    if (!url || failed.has(url)) {
      setBody(null);
      return;
    }
    const hit = known(url);
    setBody(hit?.body ?? null);
    if (hit?.fresh) return;
    let cancelled = false;
    void resolveIcon(url).then(next => {
      if (cancelled) return;
      if (next) setBody(next);
      else if (!hit) {
        failed.add(url);
        setBody(null);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [url]);

  const dims = { width: size, height: size };

  return (
    <View
      className="items-center justify-center overflow-hidden border border-rule bg-neutral-soft"
      style={dims}
    >
      {body?.kind === 'svg' || (painted !== null && painted === url) ? null : (
        <Text
          className="font-mono uppercase text-soft"
          style={{ fontSize: Math.max(TYPE.hint, Math.round(size * 0.38)) }}
        >
          {name.charAt(0)}
        </Text>
      )}
      {body?.kind === 'svg' ? (
        <SvgXml xml={body.xml} width={size} height={size} />
      ) : body?.kind === 'raster' && url ? (
        <Image
          source={{ uri: url }}
          style={[dims, { position: 'absolute', top: 0, left: 0 }]}
          resizeMode="cover"
          onLoad={() => setPainted(url)}
          onError={() => {
            failed.add(url);
            setBody(null);
          }}
        />
      ) : null}
    </View>
  );
}
