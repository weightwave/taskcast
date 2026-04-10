/**
 * Parse a boolean environment variable.
 *
 * Recognizes truthy values: "1", "true", "yes", "on" (case-insensitive, trimmed).
 * Recognizes falsy values: "0", "false", "no", "off", "", and undefined.
 *
 * @param value - The string value to parse, or undefined
 * @returns true if value matches a truthy string, false otherwise
 */
export function parseBooleanEnv(value: string | undefined): boolean {
  if (value === undefined) {
    return false
  }

  const trimmed = value.trim().toLowerCase()

  return trimmed === '1' || trimmed === 'true' || trimmed === 'yes' || trimmed === 'on'
}
