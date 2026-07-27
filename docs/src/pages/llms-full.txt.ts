import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';
import { entryToMarkdown } from '../lib/doc-markdown';

export const GET: APIRoute = async () => {
  const docs = (await getCollection('docs'))
    .filter((e) => e.data.template !== 'splash')
    .sort((a, b) => a.id.localeCompare(b.id));
  const header =
    '# Nulang — Full Documentation\n\n' +
    '> A distributed, actor-based programming language with algebraic effects, ' +
    'row-polymorphic types, and reference capabilities. Built in Rust.\n\n' +
    'Source: https://nulang.org — one section per documentation page.\n\n';
  const body = docs.map((e) => entryToMarkdown(e)).join('\n\n---\n\n');
  return new Response(header + body, {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
};
