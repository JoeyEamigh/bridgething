#!/usr/bin/env bun
import { bridgethingPush } from '@bridgething/webapp-shared/push';

bridgethingPush({ scriptUrl: import.meta.url }).catch(err => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
