export type Selection = { saved: ReadonlySet<string>; picked: ReadonlySet<string> };

export const NO_SELECTION: Selection = { saved: new Set(), picked: new Set() };

export function splitIds(value: string | null): string[] {
  return (value ?? '')
    .split(',')
    .map(s => s.trim())
    .filter(Boolean);
}

export function edited({ saved, picked }: Selection): boolean {
  return picked.size !== saved.size || [...picked].some(id => !saved.has(id));
}

export function stored(held: Selection, ids: readonly string[]): Selection {
  const saved = new Set(ids);
  return { saved, picked: edited(held) ? held.picked : saved };
}

export function fromDevice(held: Selection, value: string | null): Selection {
  return stored(held, splitIds(value));
}

export function toggled(held: Selection, id: string): Selection {
  const picked = new Set(held.picked);
  if (picked.has(id)) picked.delete(id);
  else picked.add(id);
  return { ...held, picked };
}
