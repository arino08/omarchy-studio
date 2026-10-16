// Renders every diagrams/*.html to ../assets/diagrams/<name>.png with Chromium.
// Mermaid diagrams (<pre class="mermaid">) need MERMAID_JS pointing at mermaid.min.js.
//   MERMAID_JS=/path/to/mermaid.min.js node render.js
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

(async () => {
  const dir = __dirname;
  const out = path.join(dir, '..', 'assets', 'diagrams');
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || undefined });
  const page = await browser.newPage({ deviceScaleFactor: 2, viewport: { width: 1700, height: 1200 } });
  const only = process.argv.slice(2);
  for (const f of fs.readdirSync(dir).filter((f) => f.endsWith('.html'))) {
    const name = f.replace(/\.html$/, '');
    if (only.length && !only.includes(name)) continue;
    await page.goto('file://' + path.join(dir, f));
    if (await page.$('pre.mermaid')) {
      await page.addScriptTag({ path: process.env.MERMAID_JS });
      await page.evaluate(async () => {
        mermaid.initialize({ startOnLoad: false, theme: 'base', securityLevel: 'loose',
          themeVariables: { fontFamily: 'DejaVu Sans, Arial, sans-serif', fontSize: '17px' },
          flowchart: { htmlLabels: true, curve: 'basis', padding: 14, nodeSpacing: 40, rankSpacing: 48 } });
        await mermaid.run({ querySelector: 'pre.mermaid' });
      });
    }
    await page.waitForTimeout(150);
    const el = await page.$('#diagram');
    await el.screenshot({ path: path.join(out, name + '.png') });
    console.log('rendered', name);
  }
  await browser.close();
})();
