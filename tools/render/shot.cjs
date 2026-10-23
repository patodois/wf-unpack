// Renders a glTF written by wf-unpack in headless Chromium and saves a PNG.
//
//   npm install            (once, in this folder)
//   node shot.cjs OUT.png PATH/TO/model.gltf "pos=0,5,0&tgt=0,0,10&fov=60&game=1" [WIDTHxHEIGHT]
//
// The folder of the glTF is served as /model/, this folder as /. The query is
// passed to view.html (see the comments there for the parameters). The browser
// is the one Playwright installed (PLAYWRIGHT_CHROMIUM or the headless shell
// under ~/Library/Caches/ms-playwright); set CHROME to override.
const http = require('http'), fs = require('fs'), path = require('path');
const { chromium } = require('playwright-core');
const here = __dirname;
const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.gltf': 'model/gltf+json', '.bin': 'application/octet-stream', '.png': 'image/png', '.hdr': 'application/octet-stream', '.json': 'application/json' };
function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const base = path.join(process.env.HOME || '', 'Library/Caches/ms-playwright');
  if (fs.existsSync(base)) {
    for (const d of fs.readdirSync(base).filter((n) => n.startsWith('chromium_headless_shell')).sort().reverse()) {
      const p = path.join(base, d, 'chrome-headless-shell-mac-arm64/chrome-headless-shell');
      if (fs.existsSync(p)) return p;
    }
  }
  return undefined;
}
(async () => {
  const [out, model, query, size] = process.argv.slice(2);
  if (!out || !model) { console.error('usage: node shot.cjs OUT.png model.gltf "query" [WxH]'); process.exit(2); }
  const modelAbs = path.resolve(model), modelDir = path.dirname(modelAbs);
  const server = http.createServer((req, res) => {
    const url = decodeURIComponent(req.url.split('?')[0]);
    let root = here, rel = url;
    if (url.startsWith('/model/')) { root = modelDir; rel = url.slice('/model'.length); }
    const p = path.join(root, rel);
    if (!p.startsWith(root) || !fs.existsSync(p) || fs.statSync(p).isDirectory()) { res.writeHead(404); return res.end(); }
    res.writeHead(200, { 'content-type': types[path.extname(p)] || 'application/octet-stream' });
    fs.createReadStream(p).pipe(res);
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const port = server.address().port;
  const browser = await chromium.launch({ executablePath: findChrome(), args: ['--use-angle=metal', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist'] });
  const [w, h] = (size || '1280x720').split('x').map(Number);
  const page = await browser.newPage({ viewport: { width: w, height: h } });
  let nmsg = 0;
  page.on('console', (m) => { if ((m.type() === 'error' || m.type() === 'warning') && nmsg++ < 6) console.log('console', m.type(), m.text().slice(0, 300)); });
  page.on('pageerror', (e) => console.log('pageerror', String(e).slice(0, 400)));
  await page.goto(`http://127.0.0.1:${port}/view.html?m=${encodeURIComponent('/model/' + path.basename(modelAbs))}&${query || ''}`);
  await page.waitForFunction('window.done === true', null, { timeout: 180000 });
  console.log(JSON.stringify(await page.evaluate('window.result')));
  await page.screenshot({ path: out });
  await browser.close(); server.close();
})();
