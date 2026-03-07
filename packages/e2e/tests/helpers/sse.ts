export interface SSEMessage {
  event: string
  data: string
  id?: string
}

/**
 * Collects SSE messages from a fetch Response body.
 * Parses `event:`, `data:`, and `id:` lines separated by `\n\n`.
 * Resolves when the stream closes.
 */
export async function collectSSE(
  response: Response,
  opts?: { signal?: AbortSignal },
): Promise<SSEMessage[]> {
  const messages: SSEMessage[] = []
  const reader = response.body!.getReader()
  const decoder = new TextDecoder()
  let buffer = ''

  try {
    // eslint-disable-next-line no-constant-condition
    while (true) {
      if (opts?.signal?.aborted) break
      const { done, value } = await reader.read()
      if (done) break

      buffer += decoder.decode(value, { stream: true })

      // Split on double newline (SSE message boundary)
      const parts = buffer.split('\n\n')
      // Last element is incomplete — keep it in the buffer
      buffer = parts.pop()!

      for (const part of parts) {
        if (!part.trim()) continue
        const msg: SSEMessage = { event: '', data: '' }
        for (const line of part.split('\n')) {
          if (line.startsWith('event:')) msg.event = line.slice(6).trim()
          else if (line.startsWith('data:')) msg.data = line.slice(5).trim()
          else if (line.startsWith('id:')) msg.id = line.slice(3).trim()
        }
        if (msg.event || msg.data) messages.push(msg)
      }
    }
  } catch (err) {
    // AbortError is expected when we cancel the stream
    if (!(err instanceof DOMException && err.name === 'AbortError')) throw err
  } finally {
    reader.releaseLock()
  }

  return messages
}
