import { test, expect } from '@playwright/test'

const API_BASE = 'http://localhost:3799'

test.describe('Tasks Page', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
    // Wait for auto-connect
    await expect(page.locator('h2', { hasText: 'Overview' })).toBeVisible({ timeout: 10_000 })
    // Navigate to Tasks page
    await page.getByRole('link', { name: 'Tasks' }).click()
    await expect(page.locator('h2', { hasText: 'Tasks' })).toBeVisible()
  })

  test('shows "No tasks found" when there are no tasks', async ({ page }) => {
    await expect(page.getByText('No tasks found')).toBeVisible()
  })

  test('Create Task button is visible', async ({ page }) => {
    await expect(page.getByRole('button', { name: 'Create Task' })).toBeVisible()
  })

  test('task created via API appears in the task list', async ({ page }) => {
    // Create a task via the API
    const res = await fetch(`${API_BASE}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'e2e-test-task' }),
    })
    expect(res.ok).toBe(true)
    const task = await res.json()
    const taskId = task.id as string

    // The dashboard polls every 3 seconds; wait for the task to appear
    // The table shows the last 8 chars of the ID
    const shortId = taskId.slice(-8)
    await expect(page.getByText(shortId)).toBeVisible({ timeout: 10_000 })

    // Verify task type is shown
    await expect(page.getByText('e2e-test-task')).toBeVisible()

    // Verify status badge shows "pending"
    await expect(page.getByText('pending')).toBeVisible()
  })

  test('clicking a task row opens the detail panel', async ({ page }) => {
    // Create a task via the API
    const res = await fetch(`${API_BASE}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'e2e-detail-task', params: { hello: 'world' } }),
    })
    expect(res.ok).toBe(true)
    const task = await res.json()
    const taskId = task.id as string

    // Wait for the task to appear in the table
    const shortId = taskId.slice(-8)
    await expect(page.getByText(shortId)).toBeVisible({ timeout: 10_000 })

    // Click on the task row
    await page.getByText(shortId).click()

    // Detail panel should show the full task ID
    await expect(page.getByText(taskId)).toBeVisible({ timeout: 5_000 })

    // Detail panel should show the "Info" card
    await expect(page.getByText('Info')).toBeVisible()

    // URL should update to /tasks/<taskId>
    await expect(page).toHaveURL(new RegExp(`/tasks/${taskId}`))
  })

  test('task detail shows params when present', async ({ page }) => {
    // Create a task with params
    const res = await fetch(`${API_BASE}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'e2e-params-task', params: { key: 'test-value-123' } }),
    })
    expect(res.ok).toBe(true)
    const task = await res.json()
    const taskId = task.id as string

    // Wait for the task to appear and click it
    const shortId = taskId.slice(-8)
    await expect(page.getByText(shortId)).toBeVisible({ timeout: 10_000 })
    await page.getByText(shortId).click()

    // Detail panel should show the Params card with the actual value
    await expect(page.getByText('Params')).toBeVisible({ timeout: 5_000 })
    await expect(page.getByText('test-value-123')).toBeVisible()
  })
})