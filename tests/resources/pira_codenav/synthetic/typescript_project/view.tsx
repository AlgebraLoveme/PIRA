import type { User } from "./model";

export function UserName({ user }: { user: User }) {
  return <span>{user.name}</span>;
}
