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
}
