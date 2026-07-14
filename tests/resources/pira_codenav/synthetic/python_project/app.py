from pathlib import Path

from package.api import Client


def main() -> None:
    client = Client(Path.cwd())
    print(client.fetch("users.json"))


if __name__ == "__main__":
    main()
