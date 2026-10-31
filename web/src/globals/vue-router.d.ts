import 'vue-router'

declare module 'vue-router' {
  interface RouteMeta {
    /** Reachable without a session. */
    public?: boolean
    title?: string
  }
}

export {}
