import { test, expect } from '@playwright/test'

test.describe('Sidebar Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
    // Wait for auto-connect to complete
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible({ timeout: 10_000 })
  })

  test('navigates from Overview to Tasks via sidebar', async ({ page }) => {
    await page.getByRole('link', { name: 'Tasks' }).click()
    await expect(page.locator('h2', { hasText: 'Tasks' })).toBeVisible()
    // URL should update
    await expect(page).toHaveURL(/\/tasks/)
  })

  test('navigates from Tasks to Events via sidebar', async ({ page }) => {
    await page.getByRole('link', { name: 'Tasks' }).click()
    await expect(page.locator('h2', { hasText: 'Tasks' })).toBeVisible()

    await page.getByRole('link', { name: 'Events' }).click()
    await expect(page.locator('h2', { hasText: 'Events' })).toBeVisible()
    await expect(page).toHaveURL(/\/events/)
  })

  test('navigates from Events to Workers via sidebar', async ({ page }) => {
    await page.getByRole('link', { name: 'Events' }).click()
    await expect(page.locator('h2', { hasText: 'Events' })).toBeVisible()

    await page.getByRole('link', { name: 'Workers' }).click()
    await expect(page.locator('h2', { hasText: 'Workers' })).toBeVisible()
    await expect(page).toHaveURL(/\/workers/)
  })

  test('navigates back to Overview from Workers via sidebar', async ({ page }) => {
    await page.getByRole('link', { name: 'Workers' }).click()
    await expect(page.locator('h2', { hasText: 'Workers' })).toBeVisible()

    await page.getByRole('link', { name: 'Overview' }).click()
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible()
    await expect(page).toHaveURL(/\/$/)
  })

  test('full navigation cycle: Overview → Tasks → Events → Workers → Overview', async ({ page }) => {
    // Start at Overview
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible()

    // → Tasks
    await page.getByRole('link', { name: 'Tasks' }).click()
    await expect(page.locator('h2', { hasText: 'Tasks' })).toBeVisible()

    // → Events
    await page.getByRole('link', { name: 'Events' }).click()
    await expect(page.locator('h2', { hasText: 'Events' })).toBeVisible()

    // → Workers
    await page.getByRole('link', { name: 'Workers' }).click()
    await expect(page.locator('h2', { hasText: 'Workers' })).toBeVisible()

    // → Overview
    await page.getByRole('link', { name: 'Overview' }).click()
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible()
  })

  test('direct URL access works for /tasks (SPA routing)', async ({ page }) => {
    await page.goto('/tasks')
    await expect(page.locator('h2', { hasText: 'Tasks' })).toBeVisible({ timeout: 10_000 })
  })

  test('direct URL access works for /events (SPA routing)', async ({ page }) => {
    await page.goto('/events')
    await expect(page.locator('h2', { hasText: 'Events' })).toBeVisible({ timeout: 10_000 })
  })

  test('direct URL access works for /workers (SPA routing)', async ({ page }) => {
    await page.goto('/workers')
    await expect(page.locator('h2', { hasText: 'Workers' })).toBeVisible({ timeout: 10_000 })
  })
})