import { DIRECTORY_ORIGIN, submitSource } from '@bridgething/catalog';
import { publicBase, readSource } from './lib.ts';
import { CATALOG_FILE, fail } from './paths.ts';

export async function submit(): Promise<void> {
  const url = `${publicBase(await readSource())}/${CATALOG_FILE}`;

  let outcome: Awaited<ReturnType<typeof submitSource>>;
  try {
    outcome = await submitSource(url);
  } catch (err) {
    fail(`submitting ${url} failed: ${err instanceof Error ? err.message : String(err)}`);
  }

  const { entry, created } = outcome;
  console.log(`${created ? 'submitted' : 'refreshed'} ${url}`);
  console.log(`${entry.name} is ${entry.status} in the store at ${DIRECTORY_ORIGIN}/apps`);
}
