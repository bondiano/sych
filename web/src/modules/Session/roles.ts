import type { Schemas } from '@/modules/Api'

export type Role = Schemas['ApiRole']

const ORDER: readonly Role[] = ['reader', 'triage', 'writer', 'maintainer']

/**
 * Whether the caller holds at least `role` on a repository. Only for showing or hiding an action:
 * the server decides, and checks a person's change with GitHub again.
 */
export function can(me: Schemas['Me'] | undefined, role: Role, repositoryId: number): boolean {
  const grant = me?.repositories.find((g) => g.repository.id === repositoryId)
  return grant !== undefined && ORDER.indexOf(grant.role) >= ORDER.indexOf(role)
}
