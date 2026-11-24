import { Timers } from "./timers";

export class Tasks {
  count = 0;

  spawn(timers: Timers): void {
    timers.arm();
    this.count += 1;
  }
}

export function tick(tasks: Tasks, timers: Timers): void {
  timers.fire();
  tasks.count += 1;
}
