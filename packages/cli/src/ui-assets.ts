import { fileURLToPath } from 'node:url'

export const dashboardDistPath = fileURLToPath(new URL('./ui/dashboard', import.meta.url))
export const playgroundDistPath = fileURLToPath(new URL('./ui/playground', import.meta.url))
