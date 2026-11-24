import type { Tasks } from "./index";

export class Timers {
  armed = 0;

  arm(): void {
    this.armed += 1;
  }

  fire(tasks: Tasks): void {
    tasks.count += 1;
  }
}
