import type { Schemas } from '@/modules/Api'

type InstanceLink = Schemas['InstanceLink']

/** The dashboard of another Sych server; it has its own sign-in. */
export function dashboardOf(instance: InstanceLink): string {
  return `${instance.url}/ui/`
}

/** The server that answered, or nothing when no instances are configured. */
export function currentInstance(instances: InstanceLink[]): InstanceLink | undefined {
  return instances.find((i) => i.current)
}
