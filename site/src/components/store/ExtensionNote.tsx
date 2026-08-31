import { describeExtensionPermissions, extensionRepoLabel, type AppExtension } from '@bridgething/catalog';
import { webHref } from '../../lib/href';

export function ExtensionBadge() {
  return <span class="pill pill-stable self-start">native extension</span>;
}

export function ExtensionNote({ extension, source }: { extension: AppExtension; source: string | null }) {
  const grants = describeExtensionPermissions(extension);
  const repo = extensionRepoLabel(source);
  const href = webHref(source);

  return (
    <div class="border-accent/30 border-l-2 py-1 pl-4">
      <ExtensionBadge />
      <p class="m-0 mt-3 text-sm text-white/65">
        this app runs code on your computer. it reported needing these permissions:
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
