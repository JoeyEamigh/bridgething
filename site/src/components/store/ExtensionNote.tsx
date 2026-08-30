import { describeExtensionPermissions, extensionRepoLabel, type AppExtension } from '@bridgething/catalog';
import { webHref } from '../../lib/href';

export function ExtensionBadge() {
  return <span class="pill pill-stable self-start">native extension</span>;
}

export function ExtensionNote({
  extension,
  source,
  compact,
}: {
  extension: AppExtension;
  source: string | null;
  compact?: boolean;
}) {
  const grants = describeExtensionPermissions(extension);
  const repo = extensionRepoLabel(source);
  const href = webHref(source);

  if (compact) {
    return (
      <div class="border-accent/30 flex flex-col gap-1 border-l-2 pl-2.5">
        <span class="text-accent">host access: {grants.join(', ')}</span>
        {repo && href ? (
          <a href={href} class="break-all text-white/45">
            {repo}
          </a>
        ) : (
          <span class="text-warn">no repository listed</span>
        )}
      </div>
    );
  }

  return (
    <div class="border-accent/30 border-l-2 py-1 pl-4">
      <ExtensionBadge />
      <p class="m-0 mt-3 text-sm text-white/65">
        this app runs code on the computer running the desktop app, outside the browser sandbox, with this access:
      </p>
      <ul class="m-0 mt-3 flex list-none flex-col gap-1 p-0 font-mono text-sm text-white/80">
        {grants.map(grant => (
          <li key={grant}>{grant}</li>
        ))}
      </ul>
      <p class="m-0 mt-3 font-mono text-sm break-all text-white/45">
        {repo && href ? (
          <>
            read it first: <a href={href}>{repo}</a>
          </>
        ) : (
          <span class="text-warn">no repository listed</span>
        )}
      </p>
    </div>
  );
}
