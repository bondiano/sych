import { Stream } from "./kinds";

export function open(): Stream {
  return new Stream();
}
