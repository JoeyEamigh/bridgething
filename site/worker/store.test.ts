import { describe, expect, spyOn, test } from 'bun:test';
import { KEY_PREFIX, keyFor, type SourceRecord, type SourceStatus } from './directory.ts';
import { fakeKv, withListLag, withRefusedPut, withRefusedWrites, withRivalWriter } from './kv-fake.ts';
import {
  listSources,
  mergeIntoSnapshot,
  rebuildSources,
  SNAPSHOT_MERGE_ATTEMPTS,
  takeRateLimitToken,
  writeSource,
} from './store.ts';

function record(url: string, status: SourceStatus = 'quarantined'): SourceRecord {
  return {
    url,
    name: url,
    description: null,
    homepage: null,
    icon: null,
    status,
    submitted_at: '2026-07-01T00:00:00.000Z',
    reviewed_at: null,
    reviewed_by: null,
    app_count: 1,
    last_checked_at: '2026-07-20T00:00:00.000Z',
    last_check_ok: true,
    last_check_error: null,
    downloads_cors_ok: true,
    note: null,
  };
}

function captureWarnings() {
  const lines: string[] = [];
  const spy = spyOn(console, 'warn').mockImplementation((...args) => {
    lines.push(args.map(arg => String(arg)).join(' '));
  });
  return { lines, restore: () => spy.mockRestore() };
}

async function seed(count: number) {
  const kv = fakeKv();
  for (let i = 0; i < count; i += 1) {
    await writeSource(kv, record(`https://s${i}.example/catalog.json`));
  }
  await listSources(kv);
  return kv;
}

describe('listSources', () => {
  test('reads the whole directory with one kv get, not one per source', async () => {
    const kv = await seed(25);
    kv.resetCounts();

    const records = await listSources(kv);

    expect(records).toHaveLength(25);
    expect(kv.counts.get).toBe(1);
    expect(kv.counts.list).toBe(0);
  });

  test('rebuilds from the records when the snapshot is missing', async () => {
    const kv = await seed(3);
    await kv.delete('directory:snapshot');
    kv.resetCounts();

    expect(await listSources(kv)).toHaveLength(3);
    expect(kv.counts.list).toBe(1);
  });

  test('rebuilds from the records when the snapshot is corrupt', async () => {
    const kv = await seed(3);
    await kv.put('directory:snapshot', 'not json');

    expect(await listSources(kv)).toHaveLength(3);
  });

  test('reflects a write on the very next read', async () => {
    const kv = await seed(2);
    await writeSource(kv, record('https://s0.example/catalog.json', 'attested'));

    const found = (await listSources(kv)).filter(r => r.url === 'https://s0.example/catalog.json');
    expect(found).toHaveLength(1);
    expect(found[0]!.status).toBe('attested');
  });

  test('a just-written source is in the directory before kv list can enumerate it', async () => {
    const kv = withListLag(await seed(2));
    await writeSource(kv, record('https://fresh.example/catalog.json'));

    expect((await listSources(kv)).map(r => r.url)).toContain('https://fresh.example/catalog.json');
  });

  test('the first source ever written lands in the directory with no snapshot to merge into', async () => {
    const kv = withListLag(fakeKv());
    await writeSource(kv, record('https://fresh.example/catalog.json'));

    expect((await listSources(kv)).map(r => r.url)).toEqual(['https://fresh.example/catalog.json']);
  });

  test('two writes that overlap both survive instead of one clobbering the other', async () => {
    const kv = withListLag(await seed(1));

    await Promise.all([
      writeSource(kv, record('https://a.example/catalog.json')),
      writeSource(kv, record('https://b.example/catalog.json')),
    ]);

    expect((await listSources(kv)).map(r => r.url).sort()).toEqual([
      'https://a.example/catalog.json',
      'https://b.example/catalog.json',
      'https://s0.example/catalog.json',
    ]);
  });
});

describe('mergeIntoSnapshot', () => {
  test('a put that loses the race re-merges instead of leaving its record out of the snapshot', async () => {
    const kv = withRivalWriter(fakeKv(), 'directory:snapshot', [record('https://b.example/catalog.json')], 1);

    await mergeIntoSnapshot({
      kv,
      key: 'directory:snapshot',
      prefix: KEY_PREFIX,
      record: record('https://a.example/catalog.json'),
      identity: held => held.url,
    });

    expect(kv.rivals).toBe(1);
    expect((await listSources(kv)).map(r => r.url).sort()).toEqual([
      'https://a.example/catalog.json',
      'https://b.example/catalog.json',
    ]);
  });

  test('gives up after a bounded number of attempts and invalidates the snapshot for a lazy rebuild', async () => {
    const kv = withRivalWriter(fakeKv(), 'directory:snapshot', []);
    const warnings = captureWarnings();

    try {
      await mergeIntoSnapshot({
        kv,
        key: 'directory:snapshot',
        prefix: KEY_PREFIX,
        record: record('https://a.example/catalog.json'),
        identity: held => held.url,
      });
    } finally {
      warnings.restore();
    }

    expect(kv.rivals).toBe(SNAPSHOT_MERGE_ATTEMPTS);
    expect(kv.snapshot()['directory:snapshot']).toBeUndefined();
    expect(warnings.lines).toHaveLength(1);
    expect(warnings.lines[0]).toContain('directory:snapshot');
    expect(warnings.lines[0]).toContain('https://a.example/catalog.json');
  });

  test('a refused snapshot put invalidates instead of unwinding into a 500', async () => {
    const seeded = await seed(1);
    const kv = withRefusedPut(seeded, 'directory:snapshot');
    await kv.put(keyFor('https://a.example/catalog.json'), JSON.stringify(record('https://a.example/catalog.json')));
    const warnings = captureWarnings();

    try {
      await mergeIntoSnapshot({
        kv,
        key: 'directory:snapshot',
        prefix: KEY_PREFIX,
        record: record('https://a.example/catalog.json'),
        identity: held => held.url,
      });
    } finally {
      warnings.restore();
    }

    expect(seeded.snapshot()['directory:snapshot']).toBeUndefined();
    expect(warnings.lines).toHaveLength(1);
    expect(warnings.lines[0]).toContain('directory:snapshot');
    expect(warnings.lines[0]).toContain('https://a.example/catalog.json');
    expect(warnings.lines[0]).toContain('kv put rate limit');
    expect((await listSources(seeded)).map(r => r.url).sort()).toEqual([
      'https://a.example/catalog.json',
      'https://s0.example/catalog.json',
    ]);
  });

  test('a refused invalidating delete is logged rather than raised at the caller', async () => {
    const seeded = await seed(1);
    const kv = withRefusedWrites(seeded, 'directory:snapshot');
    const warnings = captureWarnings();

    try {
      await mergeIntoSnapshot({
        kv,
        key: 'directory:snapshot',
        prefix: KEY_PREFIX,
        record: record('https://a.example/catalog.json'),
        identity: held => held.url,
      });
    } finally {
      warnings.restore();
    }

    expect(warnings.lines).toHaveLength(2);
    expect(warnings.lines[1]).toContain('delete refused');
    expect(warnings.lines[1]).toContain('kv delete rate limit');
  });
});

describe('rebuildSources', () => {
  test('repairs a snapshot that drifted from the records', async () => {
    const kv = await seed(2);
    await kv.put('directory:snapshot', JSON.stringify([record('https://ghost.example/catalog.json')]));

    const rebuilt = await rebuildSources(kv);

    expect(rebuilt.map(r => r.url).sort()).toEqual([
      'https://s0.example/catalog.json',
      'https://s1.example/catalog.json',
    ]);
    expect(await listSources(kv)).toHaveLength(2);
  });

  test('ignores keys that are not source records', async () => {
    const kv = await seed(1);
    await kv.put('rl:1.2.3.4', '3');

    expect(await rebuildSources(kv)).toHaveLength(1);
  });

  test('keeps a fresh source the snapshot holds but kv list cannot enumerate yet', async () => {
    const kv = withListLag(await seed(1));
    await writeSource(kv, record('https://fresh.example/catalog.json'));

    expect((await rebuildSources(kv)).map(r => r.url).sort()).toEqual([
      'https://fresh.example/catalog.json',
      'https://s0.example/catalog.json',
    ]);
    expect((await listSources(kv)).map(r => r.url).sort()).toEqual([
      'https://fresh.example/catalog.json',
      'https://s0.example/catalog.json',
    ]);
  });

  test('drops a snapshot item whose backing record is gone', async () => {
    const kv = await seed(2);
    await kv.delete(keyFor('https://s1.example/catalog.json'));

    expect((await rebuildSources(kv)).map(r => r.url)).toEqual(['https://s0.example/catalog.json']);
  });

  test('the per-source records stay authoritative', async () => {
    const kv = await seed(1);
    expect(kv.snapshot()[keyFor('https://s0.example/catalog.json')]).toBeDefined();
  });
});

describe('takeRateLimitToken', () => {
  test('allows up to the limit then refuses', async () => {
    const kv = fakeKv();
    const results = [];
    for (let i = 0; i < 4; i += 1) results.push(await takeRateLimitToken(kv, '1.2.3.4', 3, 3600));

    expect(results).toEqual([true, true, true, false]);
  });

  test('counts each client separately', async () => {
    const kv = fakeKv();
    await takeRateLimitToken(kv, 'a', 1, 3600);

    expect(await takeRateLimitToken(kv, 'b', 1, 3600)).toBe(true);
    expect(await takeRateLimitToken(kv, 'a', 1, 3600)).toBe(false);
  });
});
