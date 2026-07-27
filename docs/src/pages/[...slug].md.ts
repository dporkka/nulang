import type { APIRoute, GetStaticPaths } from 'astro';
import { getCollection } from 'astro:content';
import { entryToMarkdown } from '../lib/doc-markdown';

export const getStaticPaths: GetStaticPaths = async () => {
  const docs = await getCollection('docs');
  return docs
    .filter((e) => e.data.template !== 'splash') // skip the splash homepage
    .map((entry) => ({ params: { slug: entry.id }, props: { entry } }));
};

export const GET: APIRoute = ({ props }) =>
  new Response(entryToMarkdown(props.entry), {
    headers: { 'Content-Type': 'text/markdown; charset=utf-8' },
  });
