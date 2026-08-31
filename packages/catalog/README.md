# @bridgething/catalog

The `catalog.v1` schema, its validator, and the rules for resolving what to
install. A catalog is the document a [bridgething](https://github.com/JoeyEamigh/bridgething)
app source publishes; the companion apps, the desktop app, and bridgething.com
all read one through this package.

```sh
bun add @bridgething/catalog
```

```ts
import { newestCompatible, validate } from '@bridgething/catalog';

const catalog = validate(await (await fetch(url)).json());
const version = newestCompatible(catalog.apps[0], deviceLibVersion);
```

`validate` checks the JSON Schema and then the cross-reference rules the schema
cannot express: app ids are unique uuidv7, a version appears once per app,
`versions[]` is newest-first by `released_at`, extension permissions are real
Deno descriptors, and an app shipping a native extension names a `github.com`
repo as its source. It throws `CatalogValidationError` listing every failure,
not the first. `validateSchema` and `validateInvariants` are exported separately.

`newestCompatible` sorts by `released_at` itself and compares
`min_libbridgething_version` against the version the device reports, leading `v`
and all. Do not pre-sort or pre-strip.

An installed app takes updates only from the source it came from. A different
source offering the same app id is a cross-grade the user picks deliberately,
never a silent update.

Publishing a source: `bun create bridgething` scaffolds one, and
[`@bridgething/source`](https://www.npmjs.com/package/@bridgething/source) is the toolkit it wires
up, which builds, hosts and validates for you. Hosting your own means
serving `Access-Control-Allow-Origin: *` on the catalog and every
`download.url`, and never changing the bytes behind a published version. The
schema ships here as `@bridgething/catalog/schema.v1.json`.

- Docs: <https://bridgething.com/docs/publishing-apps>
- Source: <https://github.com/JoeyEamigh/bridgething>
