export function equalJitterDelay(
  baseMs: number,
  capMs: number,
  attempt: number,
  random = Math.random,
): number {
  const cap = Math.min(capMs, baseMs * 2 ** Math.max(0, attempt))
  return Math.floor(cap / 2 + random() * (cap / 2))
}
