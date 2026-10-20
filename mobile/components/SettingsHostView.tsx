import { useEffect, useMemo, useRef, useState } from 'react';
import { Platform, StyleSheet, useWindowDimensions, View } from 'react-native';
import { WebView } from 'react-native-webview';

import {
  attachSettingsView,
  serveSettingsMessage,
  setSettingsScheme,
  settingsDocument,
  settingsProcessGone,
  settingsReady,
  startSettingsHost,
  useSettingsHostStore,
} from '../lib/settings-host';
import { usePalette, useScheme } from '../lib/theme';

const BACK_EDGE = Platform.OS === 'ios' ? { left: -20 } : undefined;

export function SettingsHostView() {
  const scheme = useScheme();
  const palette = usePalette();
  const window = useWindowDimensions();
  const container = useRef<View>(null);
  const webview = useRef<WebView<object>>(null);
  const [origin, setOrigin] = useState({ x: 0, y: 0 });

  const page = useSettingsHostStore(s => s.loaded.page);
  const revision = useSettingsHostStore(s => s.loaded.revision);
  const instance = useSettingsHostStore(s => s.instance);
  const frame = useSettingsHostStore(s => s.frame);
  const ready = useSettingsHostStore(settingsReady);
  const shown = frame !== null && ready;

  const source = useMemo(
    () => ({ html: settingsDocument(page, revision) }),
    [page, revision],
  );

  useEffect(() => startSettingsHost(), []);

  useEffect(() => setSettingsScheme(scheme), [scheme]);

  useEffect(
    () =>
      attachSettingsView(script => webview.current?.injectJavaScript(script)),
    [],
  );

  const place = shown
    ? {
        left: frame.x - origin.x,
        top: frame.y - origin.y,
        width: frame.width,
        height: frame.height,
      }
    : {
        left: -2 * window.width,
        top: 0,
        width: window.width,
        height: window.height,
      };

  return (
    <View
      ref={container}
      style={StyleSheet.absoluteFill}
      pointerEvents="box-none"
      onLayout={() =>
        container.current?.measureInWindow((x, y) => setOrigin({ x, y }))
      }
    >
      <View
        style={[styles.host, place]}
        collapsable={false}
        pointerEvents={shown ? 'auto' : 'none'}
        hitSlop={BACK_EDGE}
        accessibilityElementsHidden={!shown}
        importantForAccessibility={shown ? 'auto' : 'no-hide-descendants'}
      >
        <WebView<object>
          key={instance}
          ref={webview}
          source={source}
          originWhitelist={[]}
          onMessage={event => serveSettingsMessage(event.nativeEvent.data)}
          onContentProcessDidTerminate={settingsProcessGone}
          onRenderProcessGone={settingsProcessGone}
          style={[styles.fill, { backgroundColor: palette.bg }]}
        />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  host: { position: 'absolute' },
  fill: { flex: 1 },
});
