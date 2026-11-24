import { Timers } from "./timers";

export class Tasks {
  count = 0;

  spawn(timers: Timers): void {
    timers.arm();
    this.count += 1;
  }
}
