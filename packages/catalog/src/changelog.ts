const LIST_ITEM = /^\s*(?:[-*+]|\d+[.)])\s+/;
const HEADING = /^\s{0,3}#{1,6}\s+/;
const RULE = /^\s*(?:[-*_]\s*){3,}$/;

function inline(text: string): string {
  return text
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/<\/?[a-z][^>]*>/gi, '')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/(\*\*|__)(?=\S)([\s\S]*?\S)\1/g, '$2')
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, '$1')
    .replace(/(^|[^\w*])\*(?=\S)([^*]*?\S)\*(?!\w)/g, '$1$2')
    .replace(/(^|[^\w_])_(?=\S)([^_]*?\S)_(?![\w_])/g, '$1$2')
    .replace(/\\([\\`*_{}[\]()#+\-.!~])/g, '$1');
}

function block(raw: string): string {
  const lines: string[] = [];
  for (const line of raw.split('\n')) {
    if (RULE.test(line)) continue;
    if (HEADING.test(line)) {
      lines.push(line.replace(HEADING, '').trim());
      continue;
    }
    if (LIST_ITEM.test(line)) {
      const numbered = /^\s*(\d+[.)])/.exec(line);
      lines.push(`${numbered ? numbered[1] : '•'} ${line.replace(LIST_ITEM, '').trim()}`);
      continue;
    }
    const text = line.trim();
    if (!text) continue;
    if (lines.length === 0) lines.push(text);
    else lines[lines.length - 1] += ` ${text}`;
  }
  return lines.map(inline).join('\n');
}

export function changelogText(markdown: string | null | undefined): string | null {
  if (!markdown) return null;
  const text = markdown
    .replace(/\r\n?/g, '\n')
    .split(/\n\s*\n/)
    .map(block)
    .filter(Boolean)
    .join('\n\n')
    .trim();
  return text || null;
}
