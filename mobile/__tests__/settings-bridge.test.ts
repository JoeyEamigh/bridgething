import { Linking } from 'react-native';
import InAppBrowser from 'react-native-inappbrowser-reborn';

import { settingsAuthorize, settingsFetch } from '../lib/settings-bridge';

jest.mock('react-native-inappbrowser-reborn', () => ({
  __esModule: true,
  default: {
    isAvailable: jest.fn(async () => true),
    openAuth: jest.fn(),
    closeAuth: jest.fn(),
  },
}));

class FakeFileReader {
  result: string | null = null;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;

  readAsText(blob: Blob): void {
    void blob.text().then(text => {
      this.result = text;
      this.onload?.();
    });
  }

  readAsDataURL(blob: Blob): void {
    void blob.arrayBuffer().then(buffer => {
      this.result = `data:;base64,${Buffer.from(buffer).toString('base64')}`;
      this.onload?.();
    });
  }
}

const browser = InAppBrowser as unknown as {
  isAvailable: jest.Mock;
  openAuth: jest.Mock;
  closeAuth: jest.Mock;
};

const fetchMock = jest.fn();

beforeAll(() => {
  (globalThis as unknown as { FileReader: unknown }).FileReader =
    FakeFileReader;
});

beforeEach(() => {
  fetchMock.mockReset();
  globalThis.fetch = fetchMock as unknown as typeof fetch;
  browser.isAvailable.mockResolvedValue(true);
  browser.openAuth.mockReset();
  browser.closeAuth.mockReset();
  (Linking.addEventListener as jest.Mock).mockReturnValue({
    remove: jest.fn(),
  });
});

describe('settingsFetch', () => {
  it('refuses anything that is not an http url before touching the network', async () => {
    await expect(settingsFetch({ url: 'file:///etc/passwd' })).rejects.toThrow(
      /^invalid_url:/,
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('forwards method, headers and a text body, and returns a text reply', async () => {
    fetchMock.mockResolvedValue(
      new Response('{"ok":true}', {
        status: 200,
        headers: { 'content-type': 'application/json', 'x-trace': 't1' },
      }),
    );

    const reply = await settingsFetch({
      url: 'https://api.example.test/v1',
      method: 'POST',
      headers: [['authorization', 'Bearer t']],
      body: { kind: 'text', data: '{"name":"n"}' },
    });

    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('https://api.example.test/v1');
    expect(init.method).toBe('POST');
    expect((init.headers as Headers).get('authorization')).toBe('Bearer t');
    expect(init.body).toBe('{"name":"n"}');

    expect(reply.status).toBe(200);
    expect(reply.headers).toContainEqual(['x-trace', 't1']);
    expect(reply.body).toEqual({ kind: 'text', data: '{"ok":true}' });
  });

  it('decodes a base64 request body and returns a binary reply as base64', async () => {
    fetchMock.mockResolvedValue(
      new Response(new Uint8Array([1, 2, 3, 250]), {
        status: 200,
        headers: { 'content-type': 'image/png' },
      }),
    );

    const reply = await settingsFetch({
      url: 'https://api.example.test/upload',
      method: 'PUT',
      body: { kind: 'base64', data: 'AAEC' },
    });

    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(init.body).toEqual(new Uint8Array([0, 1, 2]));
    expect(reply.body).toEqual({ kind: 'base64', data: 'AQID+g==' });
  });

  it('refuses a response body over the one mebibyte cap', async () => {
    fetchMock.mockResolvedValue(
      new Response(new Uint8Array(1024 * 1024 + 1), {
        status: 200,
        headers: { 'content-type': 'application/octet-stream' },
      }),
    );

    await expect(
      settingsFetch({ url: 'https://api.example.test/big' }),
    ).rejects.toThrow(/^network: response body is 1048577 bytes/);
  });

  it('refuses a text request body over the one mebibyte cap before touching the network', async () => {
    await expect(
      settingsFetch({
        url: 'https://api.example.test/upload',
        method: 'POST',
        body: { kind: 'text', data: 'x'.repeat(1024 * 1024 + 1) },
      }),
    ).rejects.toThrow(
      'network: request body is 1048577 bytes, over the 1048576 byte cap',
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('measures a text request body in utf-8 bytes, not utf-16 units', async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));

    await settingsFetch({
      url: 'https://api.example.test/upload',
      method: 'POST',
      body: { kind: 'text', data: '\u00e9'.repeat(512 * 1024) },
    });
    expect(fetchMock).toHaveBeenCalled();

    await expect(
      settingsFetch({
        url: 'https://api.example.test/upload',
        method: 'POST',
        body: { kind: 'text', data: '\u00e9'.repeat(512 * 1024 + 1) },
      }),
    ).rejects.toThrow(
      'network: request body is 1048578 bytes, over the 1048576 byte cap',
    );
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('refuses a base64 request body whose decoded length is over the cap', async () => {
    await expect(
      settingsFetch({
        url: 'https://api.example.test/upload',
        method: 'PUT',
        body: { kind: 'base64', data: 'A'.repeat(1398108) },
      }),
    ).rejects.toThrow(
      'network: request body is 1048581 bytes, over the 1048576 byte cap',
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('tags a transport failure as a network error', async () => {
    fetchMock.mockRejectedValue(new Error('connection refused'));

    await expect(
      settingsFetch({ url: 'https://api.example.test/down' }),
    ).rejects.toThrow('network: connection refused');
  });
});

describe('settingsAuthorize', () => {
  it('returns the callback url the auth session hands back', async () => {
    browser.openAuth.mockResolvedValue({
      type: 'success',
      url: 'bridgething://oauth/callback?code=c1',
    });

    await expect(
      settingsAuthorize('https://provider.test/authorize'),
    ).resolves.toEqual({ url: 'bridgething://oauth/callback?code=c1' });

    expect(browser.openAuth).toHaveBeenCalledWith(
      'https://provider.test/authorize',
      'bridgething://oauth/callback',
      expect.anything(),
    );
  });

  it('resolves from a Linking callback when the session never returns one', async () => {
    let deliver: ((event: { url: string }) => void) | null = null;
    (Linking.addEventListener as jest.Mock).mockImplementation(
      (_event: string, handler: (event: { url: string }) => void) => {
        deliver = handler;
        return { remove: jest.fn() };
      },
    );
    browser.openAuth.mockReturnValue(new Promise(() => {}));

    const pending = settingsAuthorize('https://provider.test/authorize');
    await Promise.resolve();
    deliver!({ url: 'bridgething://oauth/callback?code=c2' });

    await expect(pending).resolves.toEqual({
      url: 'bridgething://oauth/callback?code=c2',
    });
    expect(browser.closeAuth).toHaveBeenCalled();
  });

  it('reports a dismissed session as cancelled', async () => {
    browser.openAuth.mockResolvedValue({ type: 'cancel' });

    await expect(
      settingsAuthorize('https://provider.test/authorize'),
    ).rejects.toThrow('cancelled');
  });

  it('refuses a second authorization started in the same tick as the first', async () => {
    let release: ((result: { type: 'cancel' }) => void) | null = null;
    browser.openAuth.mockReturnValue(
      new Promise(resolve => {
        release = resolve;
      }),
    );

    const first = settingsAuthorize('https://provider.test/authorize');
    const second = settingsAuthorize('https://provider.test/authorize');

    await expect(second).rejects.toThrow(/^busy:/);
    expect(browser.openAuth).toHaveBeenCalledTimes(1);

    release!({ type: 'cancel' });
    await expect(first).rejects.toThrow('cancelled');
  });

  it('clears the in-flight flag when the device has no in-app browser', async () => {
    browser.isAvailable.mockResolvedValue(false);

    await expect(
      settingsAuthorize('https://provider.test/authorize'),
    ).rejects.toThrow(/^unsupported:/);
    expect(browser.openAuth).not.toHaveBeenCalled();

    browser.isAvailable.mockResolvedValue(true);
    browser.openAuth.mockResolvedValue({
      type: 'success',
      url: 'bridgething://oauth/callback?code=c4',
    });

    await expect(
      settingsAuthorize('https://provider.test/authorize'),
    ).resolves.toEqual({ url: 'bridgething://oauth/callback?code=c4' });
  });
});
