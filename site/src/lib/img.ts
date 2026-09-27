import type { TargetedEvent } from 'preact';

export function hideOnError(event: TargetedEvent<HTMLImageElement>): void {
  event.currentTarget.style.visibility = 'hidden';
}
