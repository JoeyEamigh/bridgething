import { Linking } from 'react-native';
import InAppBrowser from 'react-native-inappbrowser-reborn';

const SPOTIFY_ID = 'spotify';
const SPOTIFY_SCHEME = 'spotify://';

let presented: { providerId: string; url: string } | null = null;

async function openedInSpotify(providerId: string, url: string) {
  if (providerId !== SPOTIFY_ID) return false;
  try {
    if (!(await Linking.canOpenURL(SPOTIFY_SCHEME))) return false;
    await Linking.openURL(url);
    return true;
  } catch {
    return false;
  }
}

export async function openSignIn(providerId: string, url: string) {
  if (await openedInSpotify(providerId, url)) return;
  const shown = { providerId, url };
  try {
    if (!(await InAppBrowser.isAvailable())) {
      await Linking.openURL(url);
      return;
    }
    presented = shown;
    await InAppBrowser.open(url, {
      animated: true,
      modalEnabled: true,
      enableDefaultShare: false,
    });
  } catch {
    return;
  } finally {
    if (presented === shown) presented = null;
  }
}

export function closeSignIn(providerId: string) {
  if (presented?.providerId !== providerId) return;
  presented = null;
  InAppBrowser.close();
}
