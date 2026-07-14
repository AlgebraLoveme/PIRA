import { User, normalizeName } from "./lib/model.js";

export const DEFAULT_NAME = "Ada";

export function main() {
  return new User(normalizeName(DEFAULT_NAME)).label();
}
