import { openSettingsSession, type SettingsHost, type SettingsSession } from './settings-protocol.js';

type PageNode = { textContent: string | null; dispatchEvent(event: object): boolean; click(): void };

export type SettingsPageWindow = {
  readonly document: {
    readonly body: { readonly textContent: string | null; appendChild(node: object): unknown };
    write(html: string): void;
    createElement(tag: 'script'): { textContent: string | null };
    querySelector(selector: string): object | null;
    querySelectorAll(selector: 'button'): Iterable<PageNode>;
  };
  readonly Event: new (type: string, init: { bubbles: boolean }) => object;
  readonly happyDOM: { close(): Promise<void> };
};

export type SettingsPageFrame<W extends SettingsPageWindow> = W & {
  ReactNativeWebView?: { postMessage: (json: string) => void };
  __bridgethingSettingsDeliver?: (json: string) => void;
};

export type SettingsPageOptions<T> = { target: T | null; bind: (target: T) => SettingsHost };

export type SettingsPage<T, W extends SettingsPageWindow> = {
  readonly frame: SettingsPageFrame<W>;
  readonly session: SettingsSession<T>;
  load(html: string): void;
  text(): string;
  until(ready: () => boolean): Promise<void>;
  enter(selector: string, value: string): void;
  press(label: string): void;
  close(): Promise<void>;
};

const MODULE_SCRIPT = /<script type="module"[^>]*>([\s\S]*?)<\/script>/;

export function settingsPage<T, W extends SettingsPageWindow>(
  window: W,
  options: SettingsPageOptions<T>,
): SettingsPage<T, W> {
  const frame = window as SettingsPageFrame<W>;
  const session = openSettingsSession<T>({
    ...options,
    deliver: message => frame.__bridgethingSettingsDeliver?.(JSON.stringify(message)),
  });
  frame.ReactNativeWebView = { postMessage: json => session.receive(json) };

  return {
    frame,
    session,
    load(html) {
      const script = MODULE_SCRIPT.exec(html);
      if (!script) throw new Error('the settings page carries no inline module script');
      frame.document.write(html.replace(script[0], ''));
      const element = frame.document.createElement('script');
      element.textContent = script[1]!;
      frame.document.body.appendChild(element);
    },
    text: () => frame.document.body.textContent ?? '',
    async until(ready) {
      for (let i = 0; i < 50 && !ready(); i += 1) {
        await new Promise(resolve => setTimeout(resolve, 10));
      }
      await new Promise(resolve => setTimeout(resolve, 20));
    },
    enter(selector, value) {
      const field = frame.document.querySelector(selector) as PageNode | null;
      if (!field) throw new Error(`the page has no ${selector}`);
      Reflect.set(Object.getPrototypeOf(field) as object, 'value', value, field);
      field.dispatchEvent(new frame.Event('input', { bubbles: true }));
    },
    press(label) {
      const button = [...frame.document.querySelectorAll('button')].find(found => found.textContent?.trim() === label);
      if (!button) throw new Error(`the page has no ${label} button`);
      button.click();
    },
    close: () => frame.happyDOM.close(),
  };
}
