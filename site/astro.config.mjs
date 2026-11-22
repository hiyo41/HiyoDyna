// @ts-check
import { defineConfig } from 'astro/config';

export default defineConfig({
  site: 'https://hiyo41.github.io',
  base: process.env.PAGES_BASE ?? '/',
  devToolbar: { enabled: false },
});
