// Browser smoke test of the whole application, driven by Playwright.
//
// Usage: node e2e/smoke.mjs <admin-invite-url> <some.pdf> <screenshot-dir>
//
// Needs a running server with the built web client (see README) and a fresh
// data directory. It walks through the main flows: admin setup, library,
// groups and grants, invites, a member reading and annotating a sheet.

import { chromium } from 'playwright';
import { mkdirSync } from 'node:fs';

const [inviteUrl, pdfPath, outDir] = process.argv.slice(2);
if (!inviteUrl || !pdfPath || !outDir) {
  console.error('usage: node e2e/smoke.mjs <admin-invite-url> <some.pdf> <screenshot-dir>');
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
const base = new URL(inviteUrl).origin;

const browser = await chromium.launch(process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {});
let step = 0;
async function shot(page, name) {
  step += 1;
  await page.screenshot({ path: `${outDir}/${String(step).padStart(2, '0')}-${name}.png` });
}
function check(condition, message) {
  if (!condition) {
    throw new Error(`check failed: ${message}`);
  }
  console.log(`ok - ${message}`);
}

const errors = [];
function watch(page) {
  page.on('pageerror', (e) => errors.push(`${e}`));
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
}

// --- Admin: accept invite, build a small library --------------------------
const admin = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const a = await admin.newPage();
watch(a);
await a.goto(inviteUrl);
await a.getByLabel('New password (at least 8 characters)').fill('admin-password-1');
await a.getByLabel('Repeat password').fill('admin-password-1');
await a.getByRole('button', { name: 'Set password and log in' }).click();
await a.getByRole('heading', { name: 'Library' }).waitFor();
check(true, 'admin set a password via the invite link and is logged in');

await a.getByRole('link', { name: 'Manage' }).click();
await a.getByPlaceholder('Instrument, e.g. Trumpet').fill('Trumpet');
await a.getByPlaceholder('Family, e.g. Brass').fill('Brass');
await a.getByRole('button', { name: 'Add', exact: true }).click();
await a.getByText('Trumpet', { exact: false }).first().waitFor();

await a.getByPlaceholder('Title').first().fill('Florentiner Marsch');
await a.getByPlaceholder('Composer').fill('Julius Fučík');
await a.getByRole('button', { name: 'Add score' }).click();
await a.getByRole('heading', { name: 'Florentiner Marsch' }).waitFor();
await a.getByPlaceholder('New part, e.g. 2nd Trumpet in Bb').fill('2nd Trumpet in Bb');
await a.getByRole('button', { name: 'Add part' }).click();
await a.locator('.part-editor').first().waitFor();
await a.locator('.part-editor label.check', { hasText: 'Trumpet' }).locator('input').check();
await a.locator('.part-editor').getByRole('button', { name: 'Save part' }).click();
await a.locator('.part-editor .ok', { hasText: 'Saved' }).waitFor();
await a.locator('.part-editor input[type=file]').setInputFiles(pdfPath);
await a.locator('.part-editor').getByText(/Version 1/).waitFor({ timeout: 30000 });
await shot(a, 'manage-score');
check(true, 'librarian created score and part and uploaded a PDF');

// A member and a group granting the trumpet parts.
await a.getByRole('link', { name: 'Users' }).click();
await a.getByPlaceholder('Username').fill('anna');
await a.getByPlaceholder('Display name').fill('Anna');
await a.getByRole('button', { name: 'Create and get invite link' }).click();
const annaInvite = await a.locator('.card.highlight input').inputValue();
check(annaInvite.includes('/invite/'), 'admin got an invite link for anna');

await a.getByRole('link', { name: 'Groups & access' }).click();
await a.getByPlaceholder('New group, e.g. Trumpets').fill('Trumpets');
await a.getByRole('button', { name: 'Add', exact: true }).click();
await a.getByRole('heading', { name: 'Trumpets' }).waitFor();
await a.locator('label.check', { hasText: 'Anna' }).locator('input').check();
await a.locator('label.check', { hasText: 'Trumpet' }).first().locator('input').check();
await a.getByRole('button', { name: 'Save', exact: true }).click();
await a.locator('.ok', { hasText: 'Saved' }).waitFor();
await shot(a, 'groups');
check(true, 'group Trumpets gets member anna and instrument Trumpet');

// A programme with the march.
await a.getByRole('link', { name: 'Library & programmes' }).click();
await a.locator('form', { has: a.getByRole('button', { name: 'Add programme' }) }).getByPlaceholder('Title').fill('Christmas concert');
await a.getByRole('button', { name: 'Add programme' }).click();
await a.getByRole('heading', { name: 'Programme' }).waitFor();
await a.locator('select').selectOption({ label: 'Florentiner Marsch' });
await a.getByRole('button', { name: 'Add', exact: true }).click();
await a.getByRole('button', { name: 'Save programme' }).click();
await a.locator('.ok', { hasText: 'Saved' }).waitFor();
check(true, 'programme created');

// --- Member: invite, library, reader, annotations -------------------------
const member = await browser.newContext({ viewport: { width: 1024, height: 1366 }, hasTouch: false });
const m = await member.newPage();
watch(m);
await m.goto(annaInvite);
await m.getByLabel('New password (at least 8 characters)').fill('anna-password-1');
await m.getByLabel('Repeat password').fill('anna-password-1');
await m.getByRole('button', { name: 'Set password and log in' }).click();
await m.getByRole('heading', { name: 'Library' }).waitFor();
await m.getByText('Florentiner Marsch').waitFor();
await shot(m, 'member-library');
check(await m.locator('.part').count() === 1, 'anna sees exactly one part');

await m.getByPlaceholder('Search title, composer, tag…').fill('fucik');
await m.waitForTimeout(600);
check(await m.getByText('Florentiner Marsch').count() === 1, 'search without diacritics finds Fučík');

await m.getByRole('link', { name: 'Programmes' }).click();
await m.getByRole('link', { name: /Christmas concert/ }).click();
await m.getByText(/All 1 sheets ready/).waitFor({ timeout: 20000 });
await shot(m, 'programme-prefetched');
check(true, 'programme sheets were prefetched into memory');

await m.locator('.part').first().click();
await m.locator('#page-canvas').waitFor();
// pdf.js sets the CSS size once the page is drawn.
await m.waitForFunction(() => {
  const c = document.getElementById('page-canvas');
  return c && c.style.width !== '';
}, null, { timeout: 20000 });
await m.locator('.page-no', { hasText: /\d+ \/ \d+/ }).waitFor();
await shot(m, 'reader');
const pages = await m.locator('.page-no').innerText();
check(/^1 \/ \d+$/.test(pages.trim()), `reader shows page counter "${pages.trim()}"`);
const total = Number(pages.split('/')[1]);

// Draw a stroke with the mouse.
await m.getByRole('button', { name: /Draw/ }).click();
const box = await m.locator('#ink-canvas').boundingBox();
await m.mouse.move(box.x + box.width * 0.3, box.y + box.height * 0.3);
await m.mouse.down();
for (let i = 1; i <= 20; i += 1) {
  await m.mouse.move(box.x + box.width * (0.3 + i * 0.015), box.y + box.height * (0.3 + Math.sin(i / 3) * 0.03));
}
await m.mouse.up();
await m.locator('.save-status', { hasText: /^Saved$/ }).waitFor({ timeout: 10000 });
await shot(m, 'drawn');
check(true, 'stroke drawn and saved');

async function redPixels(page) {
  return page.evaluate(() => {
    const c = document.getElementById('ink-canvas');
    const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
    let n = 0;
    for (let i = 0; i < d.length; i += 4) {
      if (d[i + 3] > 0 && d[i] > 150 && d[i + 1] < 80) n += 1;
    }
    return n;
  });
}
check(await redPixels(m) > 50, 'the stroke is on the ink canvas');

// A second stroke, then reload at once: the unsaved stroke must be sent
// on page hide (keepalive) instead of being lost.
await m.mouse.move(box.x + box.width * 0.3, box.y + box.height * 0.6);
await m.mouse.down();
await m.mouse.move(box.x + box.width * 0.6, box.y + box.height * 0.62, { steps: 10 });
await m.mouse.up();
check(await m.locator('.save-status').innerText() === 'Unsaved', 'second stroke is pending');

// Reload: both strokes must come back from the server.
await m.reload();
await m.waitForFunction(() => {
  const c = document.getElementById('page-canvas');
  return c && c.style.width !== '';
}, null, { timeout: 20000 });
await m.waitForTimeout(1500);
const objects = await m.evaluate(async () => {
  const sets = await (await fetch('/api/parts/1/annotation-sets')).json();
  return sets[0].pages.map((p) => p.revision);
});
check(await redPixels(m) > 50, 'the strokes survive a reload (stored on the server)');
check(objects.length === 1 && objects[0] === 2, `the pending stroke was flushed on reload (revisions ${objects})`);

// Keyboard / foot pedal page turn.
await m.getByRole('button', { name: /Done|Draw/ }).first().click().catch(() => {});
if (total > 1) {
  await m.keyboard.press('ArrowRight');
  await m.locator('.page-no', { hasText: `2 / ${total}` }).waitFor();
  check(true, 'arrow key turns to page 2');
  check(await redPixels(m) === 0, 'page 2 has its own (empty) annotation layer');
  await m.keyboard.press('PageUp');
  await m.locator('.page-no', { hasText: `1 / ${total}` }).waitFor();
}

// The admin does not see anna's annotations.
const sets = await a.evaluate(async () => {
  const r = await fetch('/api/parts/1/annotation-sets');
  return r.json();
});
check(Array.isArray(sets) && sets.length === 0, 'annotations are private (admin sees no sets of anna)');

await m.setViewportSize({ width: 390, height: 844 });
await m.goto(`${base}/`);
await m.getByRole('heading', { name: 'Library' }).waitFor();
await shot(m, 'phone-library');

// Ignored: the dev build's hot-reload socket, and the expected 401 of the
// session check before logging in.
const relevant = errors.filter((e) => !e.includes('favicon') && !e.includes('/_dioxus')
  && !e.includes('status of 401'));
check(relevant.length === 0, `no browser errors${relevant.length ? `: ${relevant.join(' | ')}` : ''}`);
await browser.close();
console.log('smoke test passed');
