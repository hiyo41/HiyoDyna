import { describe, expect, it } from 'vitest';
import { jsonLdHtml } from './jsonld';

describe('jsonLdHtml', () => {
  it('escapes script terminators without changing the data', () => {
    const value = { name: '</script><script>alert(1)</script>' };
    const html = jsonLdHtml(value);

    expect(html).not.toContain('</script>');
    expect(html).toContain('\\u003c/script>');
    expect(JSON.parse(html)).toEqual(value);
  });
});
