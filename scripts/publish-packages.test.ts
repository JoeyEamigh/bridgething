import { describe, expect, test } from 'bun:test';
import { PACKAGES, publishable, scaffoldBlockers, type Disposition } from './publish-packages.ts';

function planWith(overrides: Record<string, Disposition>): Map<string, Disposition> {
  const plan = new Map<string, Disposition>();
  for (const p of PACKAGES) plan.set(p.name, overrides[p.name] ?? 'publish');
  return plan;
}

function names(plan: Map<string, Disposition>): string[] {
  return publishable(PACKAGES, plan).map(p => p.name);
}

describe('scaffoldBlockers', () => {
  test('is empty when every scaffolded dependency is on the registry', () => {
    expect(scaffoldBlockers(planWith({}))).toEqual([]);
    expect(scaffoldBlockers(planWith({ '@bridgething/browser': 'needs-bootstrap' }))).toEqual([]);
  });

  test('names each scaffolded dependency that still needs a first publish', () => {
    expect(scaffoldBlockers(planWith({ '@bridgething/extension': 'needs-bootstrap' }))).toEqual([
      '@bridgething/extension',
    ]);
    expect(
      scaffoldBlockers(
        planWith({ '@bridgething/client': 'needs-bootstrap', '@bridgething/extension': 'needs-bootstrap' }),
      ),
    ).toEqual(['@bridgething/client', '@bridgething/extension']);
  });
});

describe('publishable', () => {
  test('publishes create-bridgething when its scaffolded dependencies are all on the registry', () => {
    expect(names(planWith({}))).toContain('create-bridgething');
  });

  test('refuses create-bridgething while a scaffolded dependency needs a first publish', () => {
    const plan = planWith({ '@bridgething/extension': 'needs-bootstrap' });

    expect(names(plan)).not.toContain('create-bridgething');
    expect(names(plan)).toContain('@bridgething/lib');
    expect(names(plan)).toContain('@bridgething/client');
  });

  test('refuses create-bridgething when the client is the one missing', () => {
    expect(names(planWith({ '@bridgething/client': 'needs-bootstrap' }))).not.toContain('create-bridgething');
  });

  test('still drops anything already published or awaiting bootstrap', () => {
    const plan = planWith({ '@bridgething/lib': 'already-published', '@bridgething/browser': 'needs-bootstrap' });

    expect(names(plan)).not.toContain('@bridgething/lib');
    expect(names(plan)).not.toContain('@bridgething/browser');
    expect(names(plan)).toContain('create-bridgething');
  });
});
