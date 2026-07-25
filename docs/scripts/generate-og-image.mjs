import { readFileSync } from 'node:fs';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import sharp from 'sharp';

const WIDTH = 1200;
const HEIGHT = 630;

// SVG with gradient background, logo, and text
const logoPath = resolve(import.meta.dirname, '..', 'src', 'assets', 'logo.svg');
const logoContent = readFileSync(logoPath, 'utf-8');

const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${WIDTH}" height="${HEIGHT}" viewBox="0 0 ${WIDTH} ${HEIGHT}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#1e3a8a"/>
      <stop offset="100%" stop-color="#09090b"/>
    </linearGradient>
  </defs>
  <rect width="${WIDTH}" height="${HEIGHT}" fill="url(#bg)"/>
  <!-- Logo centered above text -->
  <g transform="translate(${WIDTH / 2 - 40}, ${HEIGHT / 2 - 120})">
    ${logoContent.replace(/<svg[^>]*>/, '').replace('</svg>', '')
      .replace(/width="32"/, 'width="80"')
      .replace(/height="32"/, 'height="80"')
      .replace(/viewBox="0 0 32 32"/, 'viewBox="0 0 32 32"')
      .replace('stroke-width="4.5"', 'stroke-width="5"')}
  </g>
  <text x="${WIDTH / 2}" y="${HEIGHT / 2}" text-anchor="middle" font-family="Inter, system-ui, sans-serif" font-size="96" font-weight="bold" fill="#ffffff">Nulang</text>
  <text x="${WIDTH / 2}" y="${HEIGHT / 2 + 50}" text-anchor="middle" font-family="Inter, system-ui, sans-serif" font-size="28" fill="#93c5fd">A distributed, actor-based programming language</text>
</svg>`;

const outDir = resolve(import.meta.dirname, '..', 'public');
mkdirSync(outDir, { recursive: true });
const outPath = resolve(outDir, 'og-image.png');

try {
  await sharp(Buffer.from(svg)).png().toFile(outPath);
  console.log(`OG image generated: ${outPath}`);
} catch (err) {
  console.error('Failed to generate OG image:', err.message);
  process.exit(1);
}
