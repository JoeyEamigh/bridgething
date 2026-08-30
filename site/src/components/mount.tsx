import { Window } from 'happy-dom';
import { render, type ComponentChild } from 'preact';

const DOM_GLOBALS = ['document', 'Node', 'Event', 'FocusEvent', 'MouseEvent'] as const;

const EFFECT_POLL_TICKS = 200;
const EFFECT_POLL_STEP_MS = 5;

export type Mounted = {
  root: HTMLElement;
  find(selector: string): HTMLElement;
  all(selector: string): HTMLElement[];
  text(): string;
  click(selector: string): Promise<void>;
  fill(selector: string, value: string): Promise<void>;
  blur(selector: string): Promise<void>;
  submit(selector: string): Promise<void>;
  waitFor(ready: () => boolean): Promise<void>;
  unmount(): void;
};

export function mount(node: ComponentChild): Mounted {
  const window = new Window({ url: 'https://bridgething.com/appjam' });
  const globals = globalThis as unknown as Record<string, unknown>;
  const held = new Map<string, unknown>(DOM_GLOBALS.map(name => [name, globals[name]]));
  for (const name of DOM_GLOBALS) globals[name] = (window as unknown as Record<string, unknown>)[name];

  const root = window.document.createElement('div') as unknown as HTMLElement;
  window.document.body.appendChild(root as unknown as never);
  render(node, root);

  async function settle(): Promise<void> {
    await new Promise(resolve => setTimeout(resolve, 0));
  }

  function find(selector: string): HTMLElement {
    const found = root.querySelector(selector);
    if (found === null) throw new Error(`nothing matches ${selector}`);
    return found as HTMLElement;
  }

  return {
    root,
    find,
    all: selector => [...root.querySelectorAll(selector)] as HTMLElement[],
    text: () => root.textContent ?? '',
    async click(selector) {
      find(selector).dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as MouseEvent);
      await settle();
    },
    async fill(selector, value) {
      const field = find(selector) as HTMLInputElement;
      field.value = value;
      field.dispatchEvent(new window.Event('input', { bubbles: true }) as unknown as Event);
      await settle();
    },
    async blur(selector) {
      find(selector).dispatchEvent(new window.FocusEvent('blur') as unknown as FocusEvent);
      await settle();
    },
    async submit(selector) {
      find(selector).dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }) as unknown as Event);
      await settle();
    },
    async waitFor(ready) {
      for (let tick = 0; tick < EFFECT_POLL_TICKS; tick += 1) {
        if (ready()) return;
        await new Promise(resolve => setTimeout(resolve, EFFECT_POLL_STEP_MS));
      }
      throw new Error('the mounted tree never became ready');
    },
    unmount() {
      render(null, root);
      for (const [name, value] of held) globals[name] = value;
    },
  };
}
