export class User {
  constructor(name) {
    this.name = name;
  }

  label() {
    return this.name.trim();
  }
}

export function normalizeName(value) {
  return value.trim();
}
