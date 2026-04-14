import { expect, test } from '@playwright/test';

// Minimal smoke against the full Compose stack (collector + api + web).
// The test validates that:
//   1. the web container serves the bundled app,
//   2. the bundle boots in a real browser (no runtime errors),
//   3. a seeded run appears in live mode,
//   4. selecting that run loads the tree and artifact viewer.
//
// The stack is expected to be up before this test runs; CI brings it up via
// `docker compose up -d`, seeds one run via the collector, and pings healthz
// endpoints first.

test('web app boots and shows seeded live data', async ({ page }) => {
  const pageErrors: Error[] = [];
  page.on('pageerror', (err) => pageErrors.push(err));

  await page.goto('/');
  await expect(page).toHaveTitle(/ReplayKit/i);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByTestId('run-list')).toBeVisible();

  const seededRun = page.getByText('smoke test run', { exact: true });
  await expect(seededRun).toBeVisible();
  await seededRun.click();

  const treeNode = page.getByText('smoke-tool', { exact: true });
  await expect(treeNode).toBeVisible();
  await treeNode.click();

  await expect(page.getByTestId('span-inspector')).toBeVisible();
  await expect(page.getByTestId('artifact-viewer')).toBeVisible();
  await expect(page.getByTestId('artifact-viewer')).toContainText('smoke test artifact');

  expect(pageErrors, `unexpected runtime errors: ${pageErrors.map((e) => e.message).join(' | ')}`)
    .toHaveLength(0);
});
