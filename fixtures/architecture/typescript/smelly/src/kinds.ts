import { LIMIT } from "./socket";

export class Stream {}

export function fits(n: number): boolean {
  return n < LIMIT;
}
