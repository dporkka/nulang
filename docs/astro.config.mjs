import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import tailwindcss from '@astrojs/tailwind';

// https://starlight.astro.build/reference/configuration
export default defineConfig({
  site: 'https://nulang.org',
  integrations: [
    starlight({
      title: 'Nulang',
      description: 'A distributed, actor-based programming language',
      defaultLocale: 'en',
      logo: {
        src: './src/assets/logo.svg',
        alt: 'Nulang',
      },
      favicon: '/favicon.svg',
      lastUpdated: true,
      customCss: ['./src/styles/custom.css'],
      components: {
        Footer: './src/components/Footer.astro',
        Header: './src/components/Header.astro',
      },
      editLink: {
        baseUrl: 'https://github.com/dporkka/nulang/edit/main/docs/',
      },
      // Pagefind search (built-in with Starlight)
      // To migrate to Algolia, replace with:
      //   plugins: [starlightDocSearch({ appId: '...', apiKey: '...', indexName: '...' })]
      pagefind: true,
      sidebar: [
        {
          label: 'Getting Started',
          collapsed: false,
          items: [
            { label: 'Installation', link: 'getting-started/installation/' },
            { label: 'Quick Start', link: 'getting-started/quick-start/' },
          ],
        },
        {
          label: 'Language Syntax',
          collapsed: true,
          items: [
            { label: 'Syntax Basics', link: 'language/syntax/' },
            { label: 'Type System', link: 'language/types/' },
            { label: 'Algebraic Effects', link: 'language/effects/' },
          ],
        },
        {
          label: 'Distributed Actors',
          collapsed: true,
          items: [
            { label: 'Actor Model', link: 'actors/overview/' },
            { label: 'Distribution & Clustering', link: 'actors/distribution/' },
            { label: 'Supervision Trees', link: 'actors/supervision/' },
          ],
        },
        {
          label: 'Standard Library',
          collapsed: true,
          items: [
            { label: 'Overview', link: 'stdlib/overview/' },
            { label: 'IO', link: 'stdlib/io/' },
            { label: 'Int', link: 'stdlib/int/' },
            { label: 'Timer', link: 'stdlib/timer/' },
            { label: 'Signal', link: 'stdlib/signal/' },
            { label: 'LLM', link: 'stdlib/llm/' },
            { label: 'Actor', link: 'stdlib/actor/' },
            { label: 'Otp', link: 'stdlib/otp/' },
          ],
        },
        {
          label: 'AI Agents',
          collapsed: true,
          items: [
            { label: 'Overview', link: 'ai/overview/' },
            { label: 'Memory', link: 'ai/memory/' },
            { label: 'Multi-Agent Patterns', link: 'ai/multi-agent/' },
          ],
        },
        {
          label: 'Durable Workflows',
          collapsed: true,
          items: [
            { label: 'Overview', link: 'workflows/overview/' },
            { label: 'Signals, Timers & Queries', link: 'workflows/signals-timers/' },
          ],
        },
      ],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/dporkka/nulang' },
      ],
    }),
    tailwindcss(),
  ],
});
