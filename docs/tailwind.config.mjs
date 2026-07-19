/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{astro,html,js,jsx,md,mdx,ts,tsx}'],
  darkMode: ['class', '[data-theme="dark"]'],
  theme: {
    extend: {
      colors: {
        nulang: {
          50: '#eeedff',
          100: '#d9d6ff',
          200: '#b4adff',
          300: '#8a7fff',
          400: '#6c63ff',
          500: '#4a3eff',
          600: '#3a1ef0',
          700: '#2c14d4',
          800: '#2413ab',
          900: '#1a1a2e',
          950: '#0a0a0f',
        },
      },
      fontFamily: {
        mono: ['JetBrains Mono', 'Fira Code', 'monospace'],
        sans: ['Inter', 'system-ui', 'sans-serif'],
      },
    },
  },
  plugins: [],
};
