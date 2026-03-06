import { useState } from 'react'
import { useConnectionStore } from '@/stores/connection'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'

export function LoginPage() {
  const { baseUrl, error, connect } = useConnectionStore()
  const [url, setUrl] = useState(baseUrl || 'http://localhost:3721')
  const [adminToken, setAdminToken] = useState('')
  const [loading, setLoading] = useState(false)

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    setLoading(true)
    try {
      await connect(url.replace(/\/+$/, ''), adminToken)
    } catch {
      // error is set in the store
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle className="text-2xl">Taskcast Dashboard</CardTitle>
          <CardDescription>Connect to a Taskcast server to get started.</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <label htmlFor="server-url" className="text-sm font-medium">
                Server URL
              </label>
              <Input
                id="server-url"
                type="url"
                placeholder="http://localhost:3721"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                required
              />
            </div>

            <div className="flex flex-col gap-2">
              <label htmlFor="admin-token" className="text-sm font-medium">
                Admin Token
              </label>
              <Input
                id="admin-token"
                type="password"
                placeholder="Enter admin token (optional for no-auth servers)"
                value={adminToken}
                onChange={(e) => setAdminToken(e.target.value)}
              />
            </div>

            {error && (
              <p className="text-sm text-destructive">{error}</p>
            )}

            <Button type="submit" disabled={loading || !url}>
              {loading ? 'Connecting...' : 'Connect'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
