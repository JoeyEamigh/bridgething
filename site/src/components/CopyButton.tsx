import { useState } from 'preact/hooks';

export function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);

  return (
    <button
      type="button"
      class="btn btn-sm"
      onClick={() => {
        void navigator.clipboard?.writeText(value).then(() => setCopied(true));
      }}>
      {copied ? 'copied' : 'copy'}
    </button>
  );
}
