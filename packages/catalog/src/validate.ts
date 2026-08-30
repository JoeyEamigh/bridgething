import { declaresExtension, EXTENSION_SOURCE_PATTERN, extensionOf, isExtensionPermission } from './extension.ts';
import validateSchemaFn from './generated/schema.v1.validator.mjs';
import type { Catalog } from './types.ts';

const UUIDV7 = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export class CatalogValidationError extends Error {
  constructor(
    message: string,
    public readonly errors: string[],
  ) {
    super(`${message}\n  - ${errors.join('\n  - ')}`);
    this.name = 'CatalogValidationError';
  }
}

export function validateSchema(catalog: unknown): asserts catalog is Catalog {
  if (!validateSchemaFn(catalog)) {
    const errs = (validateSchemaFn.errors ?? []).map(e => `${e.instancePath || '<root>'}: ${e.message ?? 'invalid'}`);
    throw new CatalogValidationError('catalog failed schema validation', errs);
  }
}

export function validateInvariants(catalog: Catalog): void {
  const errors: string[] = [];

  const seenIds = new Map<string, string>();
  for (const app of catalog.apps) {
    const prior = seenIds.get(app.id);
    if (prior) {
      errors.push(`app id "${app.id}" used by both "${prior}" and "${app.name}"`);
    } else {
      seenIds.set(app.id, app.name);
    }

    if (!UUIDV7.test(app.id)) {
      errors.push(`app "${app.name}" id "${app.id}" is not a valid uuidv7`);
    }

    const seenVersions = new Set<string>();
    for (const v of app.versions) {
      if (seenVersions.has(v.version)) {
        errors.push(`app "${app.name}" lists version "${v.version}" more than once`);
      } else {
        seenVersions.add(v.version);
      }

      for (const permission of extensionOf(v)?.permissions ?? []) {
        if (!isExtensionPermission(permission)) {
          errors.push(
            `app "${app.name}" version "${v.version}" declares extension permission "${permission}", which is not a permission descriptor`,
          );
        }
      }
    }

    if (declaresExtension(app) && !EXTENSION_SOURCE_PATTERN.test(app.source ?? '')) {
      errors.push(
        `app "${app.name}" ships a native extension, so its source must be a github.com repo url, not ${app.source === null ? 'null' : `"${app.source}"`}`,
      );
    }

    for (let i = 1; i < app.versions.length; i++) {
      const prev = app.versions[i - 1]!;
      const cur = app.versions[i]!;
      if (prev.released_at < cur.released_at) {
        errors.push(
          `app "${app.name}" versions are not newest-first: "${prev.version}" (${prev.released_at}) precedes "${cur.version}" (${cur.released_at})`,
        );
      }
    }
  }

  if (errors.length) {
    throw new CatalogValidationError('catalog failed cross-reference invariants', errors);
  }
}

export function validate(catalog: unknown): Catalog {
  validateSchema(catalog);
  validateInvariants(catalog);
  return catalog;
}
