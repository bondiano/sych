import { Stream } from "./kinds";

export const LIMIT = 8;

export function open(): Stream {
  return new Stream();
}
