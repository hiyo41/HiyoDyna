import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';

const root = process.cwd();
const failures = [];
let checks = 0;

function check(name, condition) {
  checks += 1;
  if (!condition) failures.push(name);
}

function read(path) {
  return readFileSync(join(root, path), 'utf8');
}

if (!existsSync(join(root, 'dist', 'index.html'))) {
  console.error('Run this from site/ after `npm run build`.');
  process.exit(1);
}

const html = read('dist/index.html');
const notFound = read('dist/404.html');
const base = process.env.PAGES_BASE || '/';
const basePrefix = base.endsWith('/') ? base : `${base}/`;

check('title mentions HiyoDyna', /<title>[^<]*HiyoDyna/.test(html));
check('canonical uses the project URL', /rel="canonical" href="https:\/\/hiyo41\.github\.io\/HiyoDyna\/"/.test(html));
check('Open Graph image is absolute', /property="og:image" content="https:\/\/hiyo41\.github\.io\/HiyoDyna\/og\.png"/.test(html));
check('static Releases fallback is present', html.includes('https://github.com/hiyo41/HiyoDyna/releases'));
check('install copy points at the executable', /<code[^>]*>HiyoDyna\.exe<\/code>/.test(html));
check('download is presented as a portable exe', html.includes('Portable exe'));
check('SoftwareApplication JSON-LD is present', html.includes('"@type":"SoftwareApplication"'));
check('404 is generated', existsSync(join(root, 'dist', '404.html')));
check('404 is noindex', /name="robots" content="noindex"/.test(notFound));
check('robots.txt is copied', existsSync(join(root, 'dist', 'robots.txt')));
check('Open Graph image is copied', existsSync(join(root, 'dist', 'og.png')));
check('OFL is copied', existsSync(join(root, 'dist', 'fonts', 'OFL.txt')));

const assets = readdirSync(join(root, 'dist', '_astro'));
check('Inter Medium is emitted', assets.some((name) => /^Inter-Medium\..*\.woff2$/.test(name)));
check('Inter Display is emitted', assets.some((name) => /^InterDisplay-SemiBold\..*\.woff2$/.test(name)));

const inlineStyles = [...html.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map((match) => match[1]);
const inlineScripts = [
  ...html.matchAll(
    /<script(?![^>]*\bsrc=)(?![^>]*application\/ld\+json)[^>]*>([\s\S]*?)<\/script>/g,
  ),
].map((match) => match[1]);

const jsFiles = assets.filter((name) => name.endsWith('.js'));
const jsBytes = jsFiles.reduce((total, name) => total + statSync(join(root, 'dist', '_astro', name)).size, 0);
const externalGzip = jsFiles.reduce(
  (total, name) => total + gzipSync(readFileSync(join(root, 'dist', '_astro', name))).length,
  0,
);
const inlineGzip = inlineScripts.reduce(
  (total, source) => total + gzipSync(Buffer.from(source)).length,
  0,
);
const jsGzip = externalGzip + inlineGzip;
check(
  `JS stays under 15KB gzipped (${jsGzip} bytes; ${jsFiles.length} external, ${inlineScripts.length} inline)`,
  jsGzip < 15 * 1024,
);
check(`JS source is not empty (${jsFiles.length} files, ${jsBytes} bytes)`, jsFiles.length > 0);

const rootAbsolute = /(?:src|href)="\/(?!HiyoDyna(?:\/|"))[^"]*"/g;
check('no root-absolute links outside the base path', !rootAbsolute.test(html));

const cssFiles = assets.filter((name) => name.endsWith('.css'));
for (const name of cssFiles) {
  const css = readFileSync(join(root, 'dist', '_astro', name), 'utf8');
  check(`CSS ${name} uses the base path`, !/url\(\/(?!HiyoDyna\/)/.test(css));
}

const inlineCss = inlineStyles.join('\n');
check('inline CSS uses the base path', !/url\(\/(?!HiyoDyna\/)/.test(inlineCss));

const allCss = `${inlineCss}\n${cssFiles
  .map((name) => readFileSync(join(root, 'dist', '_astro', name), 'utf8'))
  .join('\n')}`;
const reducedBlocks = [];
for (let from = 0; ; ) {
  const index = allCss.indexOf('prefers-reduced-motion', from);
  if (index < 0) break;
  reducedBlocks.push(allCss.slice(index, index + 900));
  from = index + 'prefers-reduced-motion'.length;
}
check('reduced-motion styles ship', /prefers-reduced-motion:\s*reduce/.test(allCss));
check(
  'reduced-motion rules suppress the island halo and keep the preview halo',
  reducedBlocks.some(
    (block) =>
      /island__halo/.test(block) && /is-previewing/.test(block) && /opacity:\s*0/.test(block),
  ),
);

const jsSource = jsFiles
  .map((name) => readFileSync(join(root, 'dist', '_astro', name), 'utf8'))
  .join('\n');
check(
  'island JS honors reduced motion and owns the preview state',
  /prefers-reduced-motion:\s*reduce/.test(jsSource) && jsSource.includes('is-previewing'),
);
check(
  'compact clock is exposed before JavaScript runs',
  /island__text--compact[^>]*aria-hidden="false"/.test(html) &&
    /island__text--expanded[^>]*aria-hidden="true"/.test(html),
);
check(
  'islands expose stable accessible names',
  html.includes('aria-label="HiyoDyna clock preview"') &&
    html.includes('aria-label="HiyoDyna clock settings preview"'),
);
check('islands use real button controls', (html.match(/class="island__button"/g) ?? []).length === 2);

check('the favicon is copied', existsSync(join(root, 'dist', 'favicon.svg')));
check('the base path is applied to icons', html.includes(`href="${basePrefix}favicon.svg"`));
check('sitemap.xml is copied', existsSync(join(root, 'dist', 'sitemap.xml')));
check(
  'robots.txt points at the sitemap',
  /Sitemap: https:\/\/hiyo41\.github\.io\/HiyoDyna\/sitemap\.xml/.test(read('dist/robots.txt')),
);

if (failures.length > 0) {
  console.error(`Build check failed (${failures.length}/${checks}):`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`Build check passed (${checks} checks, ${jsGzip} bytes JS gzipped).`);
