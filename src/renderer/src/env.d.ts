/// <reference types="vite/client" />
import type { MagpieApi } from '../../shared/types'

declare global {
  interface Window {
    api: MagpieApi
  }
}
