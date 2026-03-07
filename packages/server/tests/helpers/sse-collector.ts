export interface SSEEvent {
  event: string
  data: string
  id?: string
}

/**
 * Collects SSE events from a Response stream.
 * Resolves when `count` events are collected or the stream ends.
 */
export async function collectSSEEvents(
  res: Response,
  count: number,
): Promise<SSEEvent[]> {
  const reader = res.body!.getReader()
  const decoder = new TextDecoder()
  const collected: SSEEvent[] = []
  let buffer = ''

  while (collected.length < count) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    const blocks = buffer.split('\n\n')
    buffer = blocks.pop() ?? ''
    for (const block of blocks) {
      if (!block.trim()) continue
      const lines = block.split('\n')
      const eventLine = lines.find((l) => l.startsWith('event:'))
      const dataLine = lines.find((l) => l.startsWith('data:'))
      const idLine = lines.find((l) => l.startsWith('id:'))
      if (eventLine && dataLine) {
        collected.push({
          event: eventLine.replace('event:', '').trim(),
          data: dataLine.replace('data:', '').trim(),
          ...(idLine && { id: idLine.replace('id:', '').trim() }),
        })
      }
    }
  }

  reader.cancel()
  return collected
}

/**
 * Collects ALL SSE events until the stream closes.
 * Use for terminal tasks where the server will close the connection.
 */
export async function collectAllSSEEvents(res: Response): Promise<SSEEvent[]> {
  return collectSSEEvents(res, Infinity)
}
