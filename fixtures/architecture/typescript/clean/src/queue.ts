export class Queue {
  items: string[] = [];
  limit = 3;
  private served = 0;

  full(): boolean {
    return this.items.length + this.served >= this.limit;
  }

  get view(): string[] {
    return this.items;
  }

  push(item: string): void {
    if (!this.full()) {
      this.items.push(item);
    }
  }

  size(): number {
    return this.items.length;
  }
}
