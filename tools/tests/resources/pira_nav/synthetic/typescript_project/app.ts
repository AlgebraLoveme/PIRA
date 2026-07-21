import { Store, type User } from "./model";

export const store = new Store();

export function register(user: User): void {
  function validate(candidate: User): boolean {
    return candidate.name.length > 0;
  }
  const normalized = (candidate: User): User => ({
    ...candidate,
    name: candidate.name.trim(),
  });
  if (validate(user)) {
    store.add(normalized(user));
  }
}
