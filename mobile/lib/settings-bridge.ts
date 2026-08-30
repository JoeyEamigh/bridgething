import { Base64 } from 'js-base64';
import { type EmitterSubscription, Linking, Platform } from 'react-native';
import InAppBrowser, {
  type InAppBrowserOptions,
} from 'react-native-inappbrowser-reborn';

import { isHttpUrl } from './utils';

const MAX_BODY_BYTES = 1024 * 1024;
const TEXTY = /json|text|xml|urlencoded/i;
const CALLBACK_URL = 'bridgething://oauth/callback';

const AUTH_OPTIONS: InAppBrowserOptions = Platform.select({
  ios: { ephemeralWebSession: false, animated: true, modalEnabled: true },
  default: {
    showTitle: false,
    enableUrlBarHiding: true,
    enableDefaultShare: false,
    forceCloseOnRedirection: true,
  },
});

export type WireBody = { kind: 'text' | 'base64'; data: string };
export type WireHeader = [name: string, value: string];

export type FetchVerbRequest = {
  url: string;
  method?: string;
  headers?: WireHeader[];
  body?: WireBody;
  timeoutMs?: number;
};

export type FetchVerbReply = {
  status: number;
  headers: WireHeader[];
  body: WireBody;
};

export type AuthorizeVerbReply = { url: string };

let authorizing = false;

export async function settingsFetch(
  request: FetchVerbRequest,
): Promise<FetchVerbReply> {
  if (!isHttpUrl(request.url)) throw new Error(`invalid_url: ${request.url}`);

  const body = requestBody(request.body);
  const sent = bodyLength(body);
  if (sent > MAX_BODY_BYTES) throw oversized('request', sent);

  const controller = new AbortController();
  const deadline = request.timeoutMs
    ? setTimeout(() => controller.abort(), request.timeoutMs)
    : null;

  try {
    const response = await fetch(request.url, {
      method: request.method ?? 'GET',
      headers: new Headers(request.headers ?? []),
      body,
      signal: controller.signal,
    });

    const declared = Number(response.headers.get('content-length') ?? '0');
    if (declared > MAX_BODY_BYTES) throw oversized('response', declared);

    const blob = await response.blob();
    if (blob.size > MAX_BODY_BYTES) throw oversized('response', blob.size);

    return {
      status: response.status,
      headers: headerPairs(response.headers),
      body: await replyBody(blob, response.headers.get('content-type')),
    };
  } catch (err) {
    throw fetchFailure(err, controller.signal.aborted);
  } finally {
    if (deadline) clearTimeout(deadline);
  }
}

export async function settingsAuthorize(
  url: string,
): Promise<AuthorizeVerbReply> {
  if (!isHttpUrl(url))
    throw new Error('unsupported: the authorize url must be http or https');
  if (authorizing)
    throw new Error('busy: an authorization is already in flight');

  authorizing = true;
  let deliver: ((callback: string) => void) | null = null;
  let subscription: EmitterSubscription | null = null;

  try {
    if (!(await InAppBrowser.isAvailable()))
      throw new Error('unsupported: this device has no in-app browser');

    subscription = Linking.addEventListener('url', event => {
      if (event.url.startsWith(CALLBACK_URL)) deliver?.(event.url);
    });

    const callback = await new Promise<string | null>((resolve, reject) => {
      let settled = false;
      const settle = (finish: () => void) => {
        if (settled) return;
        settled = true;
        finish();
      };

      deliver = redirected => settle(() => resolve(redirected));
      InAppBrowser.openAuth(url, CALLBACK_URL, AUTH_OPTIONS).then(
        result =>
          settle(() => resolve(result.type === 'success' ? result.url : null)),
        (err: unknown) => settle(() => reject(asError(err))),
      );
    });

    if (callback === null) throw new Error('cancelled');
    return { url: callback };
  } finally {
    if (subscription) {
      subscription.remove();
      InAppBrowser.closeAuth();
    }
    authorizing = false;
  }
}

function requestBody(body: WireBody | undefined): string | Uint8Array | null {
  if (!body) return null;
  return body.kind === 'text' ? body.data : Base64.toUint8Array(body.data);
}

function bodyLength(body: string | Uint8Array | null): number {
  if (body === null) return 0;
  return typeof body === 'string' ? utf8Length(body) : body.byteLength;
}

function utf8Length(text: string): number {
  let bytes = 0;
  for (let i = 0; i < text.length; i += 1) {
    const unit = text.charCodeAt(i);
    if (unit < 0x80) {
      bytes += 1;
    } else if (unit < 0x800) {
      bytes += 2;
    } else if (unit >= 0xd800 && unit <= 0xdbff) {
      const trail = text.charCodeAt(i + 1);
      if (trail >= 0xdc00 && trail <= 0xdfff) {
        bytes += 4;
        i += 1;
      } else {
        bytes += 3;
      }
    } else {
      bytes += 3;
    }
  }
  return bytes;
}

function headerPairs(headers: Headers): WireHeader[] {
  const pairs: WireHeader[] = [];
  headers.forEach((value, name) => pairs.push([name, value]));
  return pairs;
}

async function replyBody(
  blob: Blob,
  contentType: string | null,
): Promise<WireBody> {
  if (blob.size === 0) return { kind: 'text', data: '' };
  if (TEXTY.test(contentType ?? ''))
    return { kind: 'text', data: await readBlob(blob, 'text') };
  const dataUrl = await readBlob(blob, 'dataUrl');
  return { kind: 'base64', data: dataUrl.slice(dataUrl.indexOf(',') + 1) };
}

function readBlob(blob: Blob, as: 'text' | 'dataUrl'): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ''));
    reader.onerror = () =>
      reject(new Error('network: the response body could not be read'));
    if (as === 'text') reader.readAsText(blob);
    else reader.readAsDataURL(blob);
  });
}

function oversized(what: 'request' | 'response', bytes: number): Error {
  return new Error(
    `network: ${what} body is ${bytes} bytes, over the ${MAX_BODY_BYTES} byte cap`,
  );
}

function asError(err: unknown): Error {
  return err instanceof Error ? err : new Error(String(err));
}

function fetchFailure(err: unknown, aborted: boolean): Error {
  const failure = asError(err);
  if (/^(network|timeout|invalid_url):/.test(failure.message)) return failure;
  if (aborted) return new Error(`timeout: ${failure.message}`);
  return new Error(`network: ${failure.message}`);
}
