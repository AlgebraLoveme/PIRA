export type UserId = string;
export type TrackedUser = [symbol: symbol, user: User];

export interface InvariantBox<in out T> {
  get(): T;
  set(value: T): void;
}

export enum Status {
  Active,
  Disabled = "disabled",
}

export interface User {
  id: UserId;
  name: string;
  label(): string;
}

export class Store {
  private users: User[] = [];

  add(user: User): void {
    this.users.push(user);
  }

  all(): readonly User[] {
    return this.users;
  }
}
