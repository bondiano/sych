export class Timers {
  armed = 0;

  arm(): void {
    this.armed += 1;
  }

  fire(): void {
    this.armed -= 1;
  }
}
