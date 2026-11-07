import { Linking } from 'react-native';
import InAppBrowser from 'react-native-inappbrowser-reborn';

import { closeSignIn, openSignIn } from '../lib/auth-browser';

jest.mock('react-native-inappbrowser-reborn', () => ({
  __esModule: true,
  default: {
    isAvailable: jest.fn(async () => true),
    open: jest.fn(),
    close: jest.fn(),
  },
}));

const browser = InAppBrowser as unknown as {
  isAvailable: jest.Mock;
  open: jest.Mock;
  close: jest.Mock;
};
const linking = Linking as unknown as {
  canOpenURL: jest.Mock;
  openURL: jest.Mock;
};

const PLEX_URL = 'https://app.plex.tv/auth#?clientID=c&code=k';
const SPOTIFY_URL = 'https://accounts.spotify.com/pair';

function heldOpen() {
  let dismiss: (() => void) | null = null;
  browser.open.mockImplementation(
    () =>
      new Promise<{ type: string }>(resolve => {
        dismiss = () => resolve({ type: 'dismiss' });
      }),
  );
  return () => dismiss?.();
}

async function settled() {
  await new Promise(resolve => setImmediate(resolve));
}

beforeEach(() => {
  browser.isAvailable.mockReset().mockResolvedValue(true);
  browser.open.mockReset();
  browser.close.mockReset();
  linking.canOpenURL = jest.fn(async () => false);
  linking.openURL = jest.fn(async () => undefined);
});

describe('openSignIn', () => {
  it('shows a provider sign-in in the in-app browser', async () => {
    heldOpen();
    void openSignIn('plex', PLEX_URL);
    await settled();
    expect(browser.open).toHaveBeenCalledWith(
      PLEX_URL,
      expect.objectContaining({ modalEnabled: true }),
    );
    expect(linking.openURL).not.toHaveBeenCalled();
  });

  it('hands spotify to the spotify app when it is installed and nobody else', async () => {
    linking.canOpenURL.mockResolvedValue(true);
    await openSignIn('spotify', SPOTIFY_URL);
    expect(linking.openURL).toHaveBeenCalledWith(SPOTIFY_URL);
    expect(browser.open).not.toHaveBeenCalled();

    heldOpen();
    void openSignIn('plex', PLEX_URL);
    await settled();
    expect(browser.open).toHaveBeenCalledTimes(1);
  });

  it('falls back to the system browser without an in-app one', async () => {
    browser.isAvailable.mockResolvedValue(false);
    await openSignIn('plex', PLEX_URL);
    expect(linking.openURL).toHaveBeenCalledWith(PLEX_URL);
    expect(browser.open).not.toHaveBeenCalled();
  });
});

describe('closeSignIn', () => {
  it('closes the browser it opened for that provider', async () => {
    heldOpen();
    void openSignIn('plex', PLEX_URL);
    await settled();
    closeSignIn('plex');
    expect(browser.close).toHaveBeenCalledTimes(1);
    closeSignIn('plex');
    expect(browser.close).toHaveBeenCalledTimes(1);
  });

  it('leaves another provider s browser alone', async () => {
    heldOpen();
    void openSignIn('plex', PLEX_URL);
    await settled();
    closeSignIn('spotify');
    expect(browser.close).not.toHaveBeenCalled();
  });

  it('does nothing once the user dismissed the browser themselves', async () => {
    const dismiss = heldOpen();
    const opening = openSignIn('plex', PLEX_URL);
    await settled();
    dismiss();
    await opening;
    closeSignIn('plex');
    expect(browser.close).not.toHaveBeenCalled();
  });

  it('never closes a browser that went to the spotify app', async () => {
    linking.canOpenURL.mockResolvedValue(true);
    await openSignIn('spotify', SPOTIFY_URL);
    closeSignIn('spotify');
    expect(browser.close).not.toHaveBeenCalled();
  });
});
