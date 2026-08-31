# **SOURCE_NAME**

**SOURCE_DESCRIPTION**

Webapps for the Spotify Car Thing running [bridgething](https://bridgething.com).

## First run

1. Push this repo to `https://github.com/__REPO_OWNER__/__REPO_NAME__`.
2. In **Settings > Pages**, set the source to **Deploy from a branch**, branch `gh-pages`, folder `/ (root)`.

The catalog is published to `__BASE_URL__/catalog.v1.json`, which can be submitted to <bridgething.com/apps>

## Develop

```sh
bun run dev            # develop the app against a connected bridgething instance
bun run dev:device     # show the dev server on the car thing screen
bun run push           # build and install to the device
bun run check          # ensure the catalog is valid
```

With more than one app in `apps/` your commands must specify which one: `bun run dev __APP_SLUG__`.

Screenshot for the store listing:

```sh
bun run shot __APP_SLUG__            # grabs what is on the screen
bun run shot __APP_SLUG__ --replace  # overwrite
```

## Add another app

```sh
bun run new weather                 # a webapp
bun run new dashboard --extension   # a webapp plus a desktop-side Deno process
bun run new home --launcher         # a replacement home screen
bun run new hud --overlay           # a system overlay drawn over every webapp
```

## Ship

```sh
bun run bump __APP_SLUG__ patch -m "Fix the wind direction arrow"
git commit -am "__APP_SLUG__: fix the wind direction arrow" && git push
```

Pushing to main builds the apps and regenerates the catalog.

## Agent skill

`.claude/skills/bridgething/` holds the `/bridgething` skill.

```sh
bun run skills           # refresh it from the published create-bridgething
bun run skills --check   # check whether it is behind
```
