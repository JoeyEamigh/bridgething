import type { VNode } from 'preact';
import { useEffect, useLayoutEffect, useMemo, useRef } from 'preact/hooks';

import { serveSettingsFrame, type WarmPage, type WarmSettings } from '../settings-host.ts';

type Props<T> = {
  settings: WarmSettings<T, Blob>;
  loaded: WarmPage<T, Blob>;
  title: string;
  class?: string;
};

function SettingsDocument<T>({ settings, loaded, title, class: className }: Props<T>): VNode {
  const frame = useRef<HTMLIFrameElement>(null);
  const loads = useRef(0);
  const { page, revision } = loaded;
  const url = useMemo(() => URL.createObjectURL(page), [page]);

  useEffect(() => () => URL.revokeObjectURL(url), [url]);

  useLayoutEffect(() => {
    const port = frame.current?.contentWindow;
    if (!port) return;
    return serveSettingsFrame(window, port, settings, revision);
  }, [settings, revision]);

  const onLoad = (): void => {
    loads.current += 1;
    if (loads.current === 1) settings.settle(revision);
    else settings.reload(revision);
  };

  return (
    <iframe ref={frame} src={url} title={title} sandbox="allow-scripts allow-forms" class={className} onLoad={onLoad} />
  );
}

export function SettingsFrame<T>(props: Props<T>): VNode {
  return <SettingsDocument key={props.loaded.revision} {...props} />;
}
