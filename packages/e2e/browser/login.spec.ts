import { test, expect } from '@playwright/test'

test.describe('Login / Auto-connect', () => {
  test('auto-connects via /api/config and shows overview page', async ({ page }) => {
    await page.goto('/')

    // Should auto-connect and land on the Overview page (not login)
    // The overview page has an h2 with text "Overview"
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible({ timeout: 10_000 })
  })

  test('sidebar heading "Taskcast" is visible after auto-connect', async ({ page }) => {
    await page.goto('/')
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible({ timeout: 10_000 })

    // The sidebar has a heading "Taskcast"
    await expect(page.locator('h1', { hasText: 'Taskcast' })).toBeVisible()
  })

  test('does not show login form when /api/config provides baseUrl', async ({ page }) => {
    await page.goto('/')
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible({ timeout: 10_000 })

    // Login form elements should NOT be visible
    await expect(page.locator('text=Connect to a Taskcast server')).not.toBeVisible()
  })
})