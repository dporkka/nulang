import type { CollectionEntry } from 'astro:content';

/** Strip MDX imports and capitalized component tags, leaving clean prose/code. */
export function stripMdx(body: string): string {
  return body
    .replace(/^import\s.+from\s.+;?\s*$/gm, '') // MDX import lines
    .replace(/<\/?[A-Z][A-Za-z0-9]*\b[^>]*\/?>/g, '') // <CopyMarkdown/>, <JitDemo/>, etc.
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

/** Render a doc entry as a standalone markdown document. */
export function entryToMarkdown(entry: CollectionEntry<'docs'>): string {
  const desc = entry.data.description ? `> ${entry.data.description}\n\n` : '';
  return `# ${entry.data.title}\n\n${desc}${stripMdx(entry.body ?? '')}\n`;
}
