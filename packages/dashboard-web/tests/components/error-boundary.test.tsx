import { describe, it, expect, vi, beforeAll, afterAll } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { ErrorBoundary } from '@/components/error-boundary'

// Use a module-level flag so ThrowingComponent can be "fixed" before clicking Try Again
let shouldThrow = false

function ThrowingComponent() {
  if (shouldThrow) {
    throw new Error('Test error message')
  }
  return <div>Child content</div>
}

describe('ErrorBoundary', () => {
  // Suppress React's error boundary console.error output during tests
  const originalConsoleError = console.error
  beforeAll(() => {
    console.error = vi.fn()
  })
  afterAll(() => {
    console.error = originalConsoleError
  })

  it('renders children when no error', () => {
    shouldThrow = false
    render(
      <ErrorBoundary>
        <div>Hello World</div>
      </ErrorBoundary>,
    )

    expect(screen.getByText('Hello World')).toBeDefined()
  })

  it('shows error message on render error', () => {
    shouldThrow = true
    render(
      <ErrorBoundary>
        <ThrowingComponent />
      </ErrorBoundary>,
    )

    expect(screen.getByText('Something went wrong')).toBeDefined()
    expect(screen.getByText('Test error message')).toBeDefined()
  })

  it('recovers when Try Again is clicked', () => {
    shouldThrow = true
    render(
      <ErrorBoundary>
        <ThrowingComponent />
      </ErrorBoundary>,
    )

    expect(screen.getByText('Something went wrong')).toBeDefined()

    // "Fix" the component before clicking Try Again so it won't throw on re-render
    shouldThrow = false
    fireEvent.click(screen.getByText('Try Again'))

    // After clicking, the boundary resets hasError=false and re-renders children.
    // Since shouldThrow is now false, ThrowingComponent renders normally.
    expect(screen.getByText('Child content')).toBeDefined()
  })
})
